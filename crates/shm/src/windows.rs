//! The mapping and the two events on Windows: `Local\dante-asio-<n>-shm`,
//! `Local\dante-asio-<n>-block` (the device signals a published block) and
//! `Local\dante-asio-<n>-ready` (the driver signals returned outputs), both
//! auto-reset. `Local\`: the device process and the DAW run in the same
//! user session.

use std::io;
use std::ptr::NonNull;
use std::sync::atomic::Ordering;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, EVENT_ALL_ACCESS, OpenEventW, SetEvent, WaitForSingleObject,
};

use crate::{Header, MAGIC, Shape, VERSION};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn name(instance: u32, what: &str) -> Vec<u16> {
    wide(&format!("Local\\dante-asio-{instance}-{what}"))
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: a handle this struct owns, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

fn handle(h: HANDLE) -> io::Result<Handle> {
    if h.is_null() || h == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(Handle(h))
    }
}

/// One instance's mapping and events, from either side.
pub struct Shared {
    view: NonNull<u8>,
    shape: Shape,
    _mapping: Handle,
    block: Handle,
    ready: Handle,
}

// SAFETY: the view is shared memory accessed through atomics (the header)
// and through buffers whose ownership the seq/ack protocol hands over.
unsafe impl Send for Shared {}
// SAFETY: as above.
unsafe impl Sync for Shared {}

impl Shared {
    /// The device side: makes instance `instance`'s mapping for `shape`
    /// (zeroed by the system) with its header, and its events.
    pub fn create(instance: u32, shape: Shape, rate: u32, offset: u32) -> io::Result<Shared> {
        let size = shape.size() as u64;
        // SAFETY: a pagefile-backed mapping with a valid NUL-terminated name.
        let mapping = handle(unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                (size >> 32) as u32,
                size as u32,
                name(instance, "shm").as_ptr(),
            )
        })?;
        let shared = Self::map(mapping, shape, instance, true)?;
        let h = shared.header();
        h.version.store(VERSION, Ordering::Relaxed);
        h.rx.store(shape.rx, Ordering::Relaxed);
        h.tx.store(shape.tx, Ordering::Relaxed);
        h.block.store(shape.block, Ordering::Relaxed);
        h.rate.store(rate, Ordering::Relaxed);
        h.offset.store(offset, Ordering::Relaxed);
        h.device_pid.store(std::process::id(), Ordering::Relaxed);
        h.magic.store(MAGIC, Ordering::Release);
        Ok(shared)
    }

    /// The driver side: opens instance `instance`'s mapping and events; its
    /// shape from the header. `NotFound` while no device made them.
    pub fn open(instance: u32) -> io::Result<Shared> {
        // SAFETY: a valid NUL-terminated name.
        let mapping = handle(unsafe {
            OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, name(instance, "shm").as_ptr())
        })?;
        // The header alone first, for the shape.
        // SAFETY: a valid mapping handle; the header's bytes.
        let peek = unsafe {
            MapViewOfFile(
                mapping.0,
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                std::mem::size_of::<Header>(),
            )
        };
        if peek.Value.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the view holds a Header the device wrote.
        let h = unsafe { &*(peek.Value as *const Header) };
        let ok = h.magic.load(Ordering::Acquire) == MAGIC
            && h.version.load(Ordering::Relaxed) == VERSION;
        let shape = Shape::new(
            h.rx.load(Ordering::Relaxed),
            h.tx.load(Ordering::Relaxed),
            h.block.load(Ordering::Relaxed),
        );
        // SAFETY: the view mapped just above.
        unsafe { UnmapViewOfFile(peek) };
        let shape = match (ok, shape) {
            (true, Some(s)) => s,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "not a dante-asio mapping",
                ));
            }
        };
        Self::map(mapping, shape, instance, false)
    }

    fn map(mapping: Handle, shape: Shape, instance: u32, create: bool) -> io::Result<Shared> {
        // SAFETY: a valid mapping handle of at least `shape.size()` bytes.
        let view: MEMORY_MAPPED_VIEW_ADDRESS =
            unsafe { MapViewOfFile(mapping.0, FILE_MAP_ALL_ACCESS, 0, 0, shape.size()) };
        let view = NonNull::new(view.Value.cast::<u8>()).ok_or_else(io::Error::last_os_error)?;
        let event = |what: &str| -> io::Result<Handle> {
            let n = name(instance, what);
            handle(if create {
                // SAFETY: an auto-reset, unsignalled event with a valid name.
                unsafe { CreateEventW(std::ptr::null(), 0, 0, n.as_ptr()) }
            } else {
                // SAFETY: a valid name.
                unsafe { OpenEventW(EVENT_ALL_ACCESS, 0, n.as_ptr()) }
            })
        };
        let block = event("block")?;
        let ready = event("ready")?;
        Ok(Shared {
            view,
            shape,
            _mapping: mapping,
            block,
            ready,
        })
    }

    pub fn shape(&self) -> Shape {
        self.shape
    }

    pub fn header(&self) -> &Header {
        // SAFETY: the view starts with the Header, aligned (the system maps
        // at a 64 KiB boundary).
        unsafe { &*(self.view.as_ptr() as *const Header) }
    }

    /// One buffer's samples (see [`Shape::buffer_offset`]); `None` past the
    /// shape. The caller owns the half the seq/ack protocol gave it.
    pub fn buffer(&self, half: u32, input: bool, ch: u32) -> Option<*mut i32> {
        let o = self.shape.buffer_offset(half, input, ch)?;
        // SAFETY: inside the view (the offset and the block fit its size).
        Some(unsafe { self.view.as_ptr().add(o) }.cast::<i32>())
    }

    /// The device: a block is published.
    pub fn signal_block(&self) {
        // SAFETY: a valid event handle.
        unsafe { SetEvent(self.block.0) };
    }

    /// The driver: waits up to `ms` for a published block.
    pub fn wait_block(&self, ms: u32) -> bool {
        // SAFETY: a valid event handle.
        unsafe { WaitForSingleObject(self.block.0, ms) == WAIT_OBJECT_0 }
    }

    /// The driver: the DAW's outputs are in.
    pub fn signal_ready(&self) {
        // SAFETY: a valid event handle.
        unsafe { SetEvent(self.ready.0) };
    }

    /// The device: waits up to `ms` for the DAW's outputs.
    pub fn wait_ready(&self, ms: u32) -> bool {
        // SAFETY: a valid event handle.
        unsafe { WaitForSingleObject(self.ready.0, ms) == WAIT_OBJECT_0 }
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        // SAFETY: the view this struct mapped, unmapped once.
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view.as_ptr().cast(),
            })
        };
    }
}
