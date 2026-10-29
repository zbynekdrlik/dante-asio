//! One instance's ASIO driver object: the IASIO methods and the worker
//! thread that drives the DAW's callbacks.
//!
//! - With the instance's device process running, the buffers the DAW gets
//!   are the shared memory's two halves (no copy): per published block the
//!   worker calls the DAW on half `seq % 2`, then stores `ack` and signals
//!   ready (at once when the DAW calls `outputReady`, else when its callback
//!   returns).
//! - With no device (or a device that went quiet for 50 ms), the worker
//!   free-runs: local buffers of silence, one callback a block period, and a
//!   `kAsioResetRequest` to the DAW once a device is there, so it opens the
//!   real buffers.

use core::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use shm::{Shape, Shared};
use windows_sys::Win32::Media::timeGetTime;
use windows_sys::Win32::System::Threading::{
    AVRT_PRIORITY_CRITICAL, AvSetMmThreadCharacteristicsW, AvSetMmThreadPriority,
};

use crate::abi::*;
use crate::com::OBJECTS;
use crate::ident::{driver_name, instance_of};

/// The rate: Dante at 96 kHz.
pub const RATE: f64 = 96_000.0;
/// The shape a DAW sees while no device runs (it asks again after a reset).
pub const NO_DEVICE_SHAPE: Shape = Shape {
    rx: 8,
    tx: 8,
    block: 32,
};
/// A device silent this long is treated as gone (free-run).
const QUIET: Duration = Duration::from_millis(50);

#[repr(C)]
pub struct Driver {
    vtbl: *const IAsioVtbl,
    refs: AtomicU32,
    instance: u32,
    state: Mutex<State>,
    rt: Arc<Rt>,
}

/// What the worker and `outputReady` share.
struct Rt {
    stop: AtomicBool,
    /// The DAW called `outputReady` for the block in hand.
    ready_sent: AtomicBool,
    /// The DAW said it calls `outputReady` (it may skip it under load).
    seq_in_hand: AtomicU64,
    /// Samples handed to the DAW since `start`.
    position: AtomicU64,
    /// The `timeGetTime` ms of the last hand-over.
    systime_ms: AtomicU64,
    shm: Mutex<Option<Arc<Shared>>>,
}

#[derive(Default)]
struct State {
    error: String,
    shape: Option<Shape>,
    created: Option<Created>,
    worker: Option<JoinHandle<()>>,
}

struct Created {
    callbacks: ASIOCallbacks,
    time_info: bool,
    /// Free-run buffers (no device): [half][channel] of `block` samples.
    _local: Option<Vec<Vec<Box<[i32]>>>>,
}

// SAFETY: the callbacks are the DAW's plain function pointers, callable from
// any thread (the SDK's contract); the local buffers are owned data.
unsafe impl Send for Created {}

static VTBL: IAsioVtbl = IAsioVtbl {
    QueryInterface: query_interface,
    AddRef: add_ref,
    Release: release,
    init,
    getDriverName: get_driver_name,
    getDriverVersion: get_driver_version,
    getErrorMessage: get_error_message,
    start,
    stop,
    getChannels: get_channels,
    getLatencies: get_latencies,
    getBufferSize: get_buffer_size,
    canSampleRate: can_sample_rate,
    getSampleRate: get_sample_rate,
    setSampleRate: set_sample_rate,
    getClockSources: get_clock_sources,
    setClockSource: set_clock_source,
    getSamplePosition: get_sample_position,
    getChannelInfo: get_channel_info,
    createBuffers: create_buffers,
    disposeBuffers: dispose_buffers,
    controlPanel: control_panel,
    future,
    outputReady: output_ready,
};

impl Driver {
    /// A new driver for `instance` with one reference, as COM hands it out.
    pub(crate) fn new_raw(instance: u32) -> *mut c_void {
        OBJECTS.fetch_add(1, Ordering::AcqRel);
        Box::into_raw(Box::new(Driver {
            vtbl: &VTBL,
            refs: AtomicU32::new(1),
            instance,
            state: Mutex::new(State::default()),
            rt: Arc::new(Rt {
                stop: AtomicBool::new(false),
                ready_sent: AtomicBool::new(false),
                seq_in_hand: AtomicU64::new(0),
                position: AtomicU64::new(0),
                systime_ms: AtomicU64::new(0),
                shm: Mutex::new(None),
            }),
        }))
        .cast()
    }

    fn shape(&self) -> Shape {
        self.state.lock().unwrap().shape.unwrap_or(NO_DEVICE_SHAPE)
    }

    fn latencies(&self) -> (i32, i32) {
        let shm = self.rt.shm.lock().unwrap();
        match shm.as_ref() {
            Some(s) => {
                let h = s.header();
                (
                    h.latency_in.load(Ordering::Relaxed) as i32,
                    h.latency_out.load(Ordering::Relaxed) as i32,
                )
            }
            None => {
                let b = NO_DEVICE_SHAPE.block as i32;
                (b, b)
            }
        }
    }

    fn stop_worker(&self, state: &mut State) {
        if let Some(w) = state.worker.take() {
            self.rt.stop.store(true, Ordering::Release);
            let _ = w.join();
            self.rt.stop.store(false, Ordering::Release);
        }
    }
}

/// # Safety
/// `this` is a live Driver.
unsafe fn me<'a>(this: *mut c_void) -> &'a Driver {
    // SAFETY: the caller's.
    unsafe { &*(this as *const Driver) }
}

fn write_c(dst: *mut c_char, text: &str, cap: usize) {
    if dst.is_null() {
        return;
    }
    let bytes = text.as_bytes();
    let n = bytes.len().min(cap - 1);
    // SAFETY: the SDK gives buffers of `cap` bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr().cast::<c_char>(), dst, n);
        *dst.add(n) = 0;
    }
}

unsafe extern "system" fn query_interface(
    this: *mut c_void,
    riid: *const Guid,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if ppv.is_null() || riid.is_null() {
        return E_POINTER;
    }
    // SAFETY: COM's valid pointers; `this` a live Driver.
    let (iid, d) = unsafe { (*riid, me(this)) };
    if iid == IID_IUNKNOWN || instance_of(&iid) == Some(d.instance) {
        d.refs.fetch_add(1, Ordering::AcqRel);
        // SAFETY: checked non-null.
        unsafe { *ppv = this };
        S_OK
    } else {
        // SAFETY: checked non-null.
        unsafe { *ppv = core::ptr::null_mut() };
        E_NOINTERFACE
    }
}

unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
    // SAFETY: a live Driver.
    unsafe { me(this) }.refs.fetch_add(1, Ordering::AcqRel) + 1
}

unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    // SAFETY: a live Driver.
    let d = unsafe { me(this) };
    let left = d.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        {
            let mut st = d.state.lock().unwrap();
            d.stop_worker(&mut st);
            if let Some(s) = d.rt.shm.lock().unwrap().as_ref() {
                s.header().host_attached.store(0, Ordering::Release);
            }
        }
        // SAFETY: the last reference: the Box this driver came from.
        drop(unsafe { Box::from_raw(this as *mut Driver) });
        OBJECTS.fetch_sub(1, Ordering::AcqRel);
    }
    left
}

unsafe extern "system" fn init(this: *mut c_void, _sys: *mut c_void) -> ASIOBool {
    // SAFETY: a live Driver.
    let d = unsafe { me(this) };
    let mut st = d.state.lock().unwrap();
    match Shared::open(d.instance) {
        Ok(s) => {
            st.shape = Some(s.shape());
            *d.rt.shm.lock().unwrap() = Some(Arc::new(s));
        }
        Err(e) => {
            st.shape = None;
            st.error = format!("no dante-asio device {} running ({e})", d.instance);
        }
    }
    // A DAW must be able to open the driver without its device: it free-runs.
    ASIO_TRUE
}

unsafe extern "system" fn get_driver_name(this: *mut c_void, name: *mut c_char) {
    // SAFETY: a live Driver.
    write_c(name, &driver_name(unsafe { me(this) }.instance), 32);
}

unsafe extern "system" fn get_driver_version(_this: *mut c_void) -> i32 {
    1
}

unsafe extern "system" fn get_error_message(this: *mut c_void, text: *mut c_char) {
    // SAFETY: a live Driver.
    let e = unsafe { me(this) }.state.lock().unwrap().error.clone();
    write_c(text, &e, 124);
}

unsafe extern "system" fn start(this: *mut c_void) -> ASIOError {
    // SAFETY: a live Driver.
    let d = unsafe { me(this) };
    let mut st = d.state.lock().unwrap();
    if st.worker.is_some() {
        return ASE_OK;
    }
    let Some(created) = st.created.as_ref() else {
        return ASE_INVALID_MODE;
    };
    let job = Job {
        callbacks: created.callbacks,
        time_info: created.time_info,
        shape: st.shape.unwrap_or(NO_DEVICE_SHAPE),
        instance: d.instance,
        rt: Arc::clone(&d.rt),
    };
    d.rt.position.store(0, Ordering::Relaxed);
    match std::thread::Builder::new()
        .name("dante-asio".into())
        .spawn(move || job.run())
    {
        Ok(w) => {
            st.worker = Some(w);
            ASE_OK
        }
        Err(_) => ASE_HW_MALFUNCTION,
    }
}

unsafe extern "system" fn stop(this: *mut c_void) -> ASIOError {
    // SAFETY: a live Driver.
    let d = unsafe { me(this) };
    let mut st = d.state.lock().unwrap();
    d.stop_worker(&mut st);
    ASE_OK
}

unsafe extern "system" fn get_channels(
    this: *mut c_void,
    ins: *mut i32,
    outs: *mut i32,
) -> ASIOError {
    // SAFETY: a live Driver.
    let s = unsafe { me(this) }.shape();
    if ins.is_null() || outs.is_null() {
        return ASE_INVALID_PARAMETER;
    }
    // SAFETY: checked non-null.
    unsafe {
        *ins = s.rx as i32;
        *outs = s.tx as i32;
    }
    ASE_OK
}

unsafe extern "system" fn get_latencies(
    this: *mut c_void,
    ins: *mut i32,
    outs: *mut i32,
) -> ASIOError {
    // SAFETY: a live Driver.
    let (i, o) = unsafe { me(this) }.latencies();
    if ins.is_null() || outs.is_null() {
        return ASE_INVALID_PARAMETER;
    }
    // SAFETY: checked non-null.
    unsafe {
        *ins = i;
        *outs = o;
    }
    ASE_OK
}

unsafe extern "system" fn get_buffer_size(
    this: *mut c_void,
    min: *mut i32,
    max: *mut i32,
    preferred: *mut i32,
    granularity: *mut i32,
) -> ASIOError {
    // SAFETY: a live Driver.
    let b = unsafe { me(this) }.shape().block as i32;
    if min.is_null() || max.is_null() || preferred.is_null() || granularity.is_null() {
        return ASE_INVALID_PARAMETER;
    }
    // The instance's block only: its tray sets it (the device's pump runs it).
    // SAFETY: checked non-null.
    unsafe {
        *min = b;
        *max = b;
        *preferred = b;
        *granularity = 0;
    }
    ASE_OK
}

unsafe extern "system" fn can_sample_rate(_this: *mut c_void, rate: f64) -> ASIOError {
    if rate == RATE { ASE_OK } else { ASE_NO_CLOCK }
}

unsafe extern "system" fn get_sample_rate(_this: *mut c_void, rate: *mut f64) -> ASIOError {
    if rate.is_null() {
        return ASE_INVALID_PARAMETER;
    }
    // SAFETY: checked non-null.
    unsafe { *rate = RATE };
    ASE_OK
}

unsafe extern "system" fn set_sample_rate(_this: *mut c_void, rate: f64) -> ASIOError {
    if rate == RATE || rate == 0.0 {
        ASE_OK
    } else {
        ASE_NO_CLOCK
    }
}

unsafe extern "system" fn get_clock_sources(
    _this: *mut c_void,
    clocks: *mut ASIOClockSource,
    n: *mut i32,
) -> ASIOError {
    if clocks.is_null() || n.is_null() {
        return ASE_INVALID_PARAMETER;
    }
    // SAFETY: the host gives at least one entry and its count.
    unsafe {
        let c = &mut *clocks;
        c.index = 0;
        c.associatedChannel = -1;
        c.associatedGroup = -1;
        c.isCurrentSource = ASIO_TRUE;
        write_c(c.name.as_mut_ptr(), "Dante (PTP)", 32);
        *n = 1;
    }
    ASE_OK
}

unsafe extern "system" fn set_clock_source(_this: *mut c_void, index: i32) -> ASIOError {
    if index == 0 {
        ASE_OK
    } else {
        ASE_INVALID_PARAMETER
    }
}

unsafe extern "system" fn get_sample_position(
    this: *mut c_void,
    pos: *mut ASIOSamples,
    stamp: *mut ASIOTimeStamp,
) -> ASIOError {
    if pos.is_null() || stamp.is_null() {
        return ASE_INVALID_PARAMETER;
    }
    // SAFETY: a live Driver; checked non-null.
    let d = unsafe { me(this) };
    unsafe {
        *pos = Asio64::new(d.rt.position.load(Ordering::Acquire));
        *stamp = Asio64::new(d.rt.systime_ms.load(Ordering::Acquire) * 1_000_000);
    }
    ASE_OK
}

unsafe extern "system" fn get_channel_info(
    this: *mut c_void,
    info: *mut ASIOChannelInfo,
) -> ASIOError {
    if info.is_null() {
        return ASE_INVALID_PARAMETER;
    }
    // SAFETY: a live Driver; the host's struct with `channel` and `isInput` set.
    let (d, i) = unsafe { (me(this), &mut *info) };
    let s = d.shape();
    let (ch, input) = (i.channel, i.isInput != 0);
    let count = if input { s.rx } else { s.tx } as i32;
    if ch < 0 || ch >= count {
        return ASE_INVALID_PARAMETER;
    }
    i.isActive = ASIO_TRUE;
    i.channelGroup = 0;
    i.type_ = ASIOST_INT32_LSB;
    let name = format!("{} {}", if input { "RX" } else { "TX" }, ch + 1);
    write_c(i.name.as_mut_ptr(), &name, 32);
    ASE_OK
}

unsafe extern "system" fn create_buffers(
    this: *mut c_void,
    infos: *mut ASIOBufferInfo,
    n: i32,
    size: i32,
    callbacks: *const ASIOCallbacks,
) -> ASIOError {
    if infos.is_null() || callbacks.is_null() || n <= 0 {
        return ASE_INVALID_PARAMETER;
    }
    // SAFETY: a live Driver; the host's `n` buffer infos and its callbacks.
    let (d, infos, callbacks) = unsafe {
        (
            me(this),
            core::slice::from_raw_parts_mut(infos, n as usize),
            *callbacks,
        )
    };
    let mut st = d.state.lock().unwrap();
    d.stop_worker(&mut st);
    let shape = st.shape.unwrap_or(NO_DEVICE_SHAPE);
    if size != shape.block as i32 {
        return ASE_INVALID_MODE;
    }
    let shm = d.rt.shm.lock().unwrap().clone();
    let mut local: Vec<Vec<Box<[i32]>>> = vec![Vec::new(), Vec::new()];
    for info in infos.iter_mut() {
        let input = info.isInput != 0;
        let ch = info.channelNum;
        let count = if input { shape.rx } else { shape.tx } as i32;
        if ch < 0 || ch >= count {
            return ASE_INVALID_PARAMETER;
        }
        let ch = ch as u32;
        let mut bufs = [core::ptr::null_mut(); 2];
        for (half, b) in bufs.iter_mut().enumerate() {
            *b = match shm.as_ref() {
                Some(s) => s.buffer(half as u32, input, ch).unwrap().cast(),
                None => {
                    let mut v = vec![0i32; shape.block as usize].into_boxed_slice();
                    let p = v.as_mut_ptr().cast();
                    local[half].push(v);
                    p
                }
            };
        }
        info.buffers = bufs;
    }
    // SAFETY: the host's callback, asked as the SDK says.
    let time_info = callbacks.asioMessage.is_some_and(|m| unsafe {
        m(
            K_ASIO_SELECTOR_SUPPORTED,
            K_ASIO_SUPPORTS_TIME_INFO,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        ) == 1
            && m(
                K_ASIO_SUPPORTS_TIME_INFO,
                0,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            ) == 1
    });
    if let Some(s) = shm.as_ref() {
        s.header().host_attached.store(1, Ordering::Release);
    }
    st.created = Some(Created {
        callbacks,
        time_info,
        _local: shm.is_none().then_some(local),
    });
    ASE_OK
}

unsafe extern "system" fn dispose_buffers(this: *mut c_void) -> ASIOError {
    // SAFETY: a live Driver.
    let d = unsafe { me(this) };
    let mut st = d.state.lock().unwrap();
    d.stop_worker(&mut st);
    st.created = None;
    if let Some(s) = d.rt.shm.lock().unwrap().as_ref() {
        s.header().host_attached.store(0, Ordering::Release);
    }
    ASE_OK
}

unsafe extern "system" fn control_panel(_this: *mut c_void) -> ASIOError {
    // The settings live in the instance's tray icon.
    ASE_OK
}

unsafe extern "system" fn future(
    _this: *mut c_void,
    selector: i32,
    _opt: *mut c_void,
) -> ASIOError {
    match selector {
        K_ASIO_CAN_TIME_INFO => ASE_SUCCESS,
        _ => ASE_NOT_PRESENT,
    }
}

unsafe extern "system" fn output_ready(this: *mut c_void) -> ASIOError {
    // SAFETY: a live Driver.
    let rt = &unsafe { me(this) }.rt;
    if let Some(s) = rt.shm.lock().unwrap().as_ref() {
        s.header()
            .ack
            .store(rt.seq_in_hand.load(Ordering::Acquire), Ordering::Release);
        s.signal_ready();
        rt.ready_sent.store(true, Ordering::Release);
    }
    ASE_OK
}

/// What the worker thread runs on.
struct Job {
    callbacks: ASIOCallbacks,
    time_info: bool,
    shape: Shape,
    instance: u32,
    rt: Arc<Rt>,
}

impl Job {
    fn run(self) {
        pro_audio();
        let shm = self.rt.shm.lock().unwrap().clone();
        let mut time = ASIOTime {
            reserved: [0; 4],
            timeInfo: AsioTimeInfo {
                speed: 1.0,
                systemTime: Asio64::default(),
                samplePosition: Asio64::default(),
                sampleRate: RATE,
                flags: K_SYSTEM_TIME_VALID | K_SAMPLE_POSITION_VALID | K_SAMPLE_RATE_VALID,
                reserved: [0; 12],
            },
            timeCode: ASIOTimeCode {
                speed: 0.0,
                timeCodeSamples: Asio64::default(),
                flags: 0,
                future: [0; 64],
            },
        };
        let period = Duration::from_secs_f64(f64::from(self.shape.block) / RATE);
        let mut next = Instant::now();
        let mut half = 0i32;
        let mut checked = Instant::now();
        while !self.rt.stop.load(Ordering::Acquire) {
            match shm.as_ref() {
                Some(s) => {
                    let ms = QUIET.as_millis() as u32;
                    if !s.wait_block(ms) {
                        // A device gone quiet: the DAW keeps its clock on
                        // silence in the half it would get next.
                        half ^= 1;
                        self.silence(s, half as u32);
                        self.switch(&mut time, half);
                        continue;
                    }
                    let seq = s.header().seq.load(Ordering::Acquire);
                    half = (seq % 2) as i32;
                    self.rt.seq_in_hand.store(seq, Ordering::Release);
                    self.rt.ready_sent.store(false, Ordering::Release);
                    self.switch(&mut time, half);
                    if !self.rt.ready_sent.swap(false, Ordering::AcqRel) {
                        s.header().ack.store(seq, Ordering::Release);
                        s.signal_ready();
                    }
                }
                None => {
                    // Free-run on our own clock; a device that appears asks
                    // the DAW for a reset (it then opens the real buffers).
                    next += period;
                    let now = Instant::now();
                    if next > now {
                        sleep_until(next);
                    } else if now - next > Duration::from_millis(100) {
                        next = now;
                    }
                    half ^= 1;
                    self.switch(&mut time, half);
                    if checked.elapsed() > Duration::from_secs(1) {
                        checked = Instant::now();
                        if Shared::open(self.instance).is_ok() {
                            self.message(K_ASIO_RESET_REQUEST);
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Zeroes the inputs of `half` (a device gone quiet).
    fn silence(&self, s: &Shared, half: u32) {
        for ch in 0..self.shape.rx {
            if let Some(p) = s.buffer(half, true, ch) {
                // SAFETY: a buffer of `block` samples in the mapping; the
                // device is not writing it (it went quiet).
                unsafe { core::ptr::write_bytes(p, 0, self.shape.block as usize) };
            }
        }
    }

    fn message(&self, selector: i32) {
        if let Some(m) = self.callbacks.asioMessage {
            // SAFETY: the host's callback.
            unsafe { m(selector, 0, core::ptr::null_mut(), core::ptr::null_mut()) };
        }
    }

    /// Hands half `half` to the DAW.
    fn switch(&self, time: &mut ASIOTime, half: i32) {
        let pos = self
            .rt
            .position
            .fetch_add(u64::from(self.shape.block), Ordering::AcqRel);
        // SAFETY: plain call.
        let ms = u64::from(unsafe { timeGetTime() });
        self.rt.systime_ms.store(ms, Ordering::Release);
        time.timeInfo.samplePosition = Asio64::new(pos);
        time.timeInfo.systemTime = Asio64::new(ms * 1_000_000);
        if self.time_info {
            if let Some(f) = self.callbacks.bufferSwitchTimeInfo {
                // SAFETY: the host's callback with a valid ASIOTime.
                unsafe { f(time, half, ASIO_TRUE) };
                return;
            }
        }
        if let Some(f) = self.callbacks.bufferSwitch {
            // SAFETY: the host's callback.
            unsafe { f(half, ASIO_TRUE) };
        }
    }
}

/// The calling thread into MMCSS "Pro Audio" at critical priority (the
/// ASIO SDK's rule for a driver's audio thread).
fn pro_audio() {
    let task: Vec<u16> = "Pro Audio".encode_utf16().chain(Some(0)).collect();
    let mut index = 0u32;
    // SAFETY: a NUL-terminated name and a writable index; the registration
    // is kept for the thread's life.
    let h = unsafe { AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut index) };
    if !h.is_null() {
        // SAFETY: the registration just made.
        unsafe { AvSetMmThreadPriority(h, AVRT_PRIORITY_CRITICAL) };
    }
}

/// Sleeps until `t`: whole milliseconds by sleep, the rest spun.
fn sleep_until(t: Instant) {
    loop {
        let now = Instant::now();
        if now >= t {
            return;
        }
        let left = t - now;
        if left > Duration::from_millis(2) {
            std::thread::sleep(left - Duration::from_millis(1));
        } else {
            std::hint::spin_loop();
        }
    }
}
