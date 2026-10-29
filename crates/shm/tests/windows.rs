#![cfg(windows)]

use std::sync::atomic::Ordering;

use shm::{MAGIC, Shape, Shared};

#[test]
fn create_then_open_shares_the_buffers_and_the_header() {
    let shape = Shape::new(4, 2, 64).unwrap();
    let device = Shared::create(91, shape, 96_000, 208).unwrap();
    let driver = Shared::open(91).unwrap();
    assert_eq!(driver.shape(), shape);
    assert_eq!(driver.header().magic.load(Ordering::Acquire), MAGIC);
    assert_eq!(driver.header().offset.load(Ordering::Relaxed), 208);
    let input = device.buffer(1, true, 3).unwrap();
    // SAFETY: a buffer of 64 samples inside the mapping.
    unsafe { input.add(63).write(0x7f00_0000) };
    let seen = driver.buffer(1, true, 3).unwrap();
    // SAFETY: the same buffer through the other view.
    assert_eq!(unsafe { seen.add(63).read() }, 0x7f00_0000);
    device.header().seq.store(7, Ordering::Release);
    assert_eq!(driver.header().seq.load(Ordering::Acquire), 7);
}

#[test]
fn the_events_cross_between_the_sides() {
    let shape = Shape::new(1, 1, 32).unwrap();
    let device = Shared::create(92, shape, 96_000, 128).unwrap();
    let driver = Shared::open(92).unwrap();
    assert!(!driver.wait_block(10), "nothing published yet");
    device.signal_block();
    assert!(driver.wait_block(1000));
    assert!(!driver.wait_block(10), "auto-reset: one signal, one wake");
    driver.signal_ready();
    assert!(device.wait_ready(1000));
}

#[test]
fn opening_an_instance_no_device_made_is_not_found() {
    let e = Shared::open(93).err().unwrap();
    assert_eq!(e.kind(), std::io::ErrorKind::NotFound, "{e}");
}
