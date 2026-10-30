//! The whole path as on the PC, on a GitHub-hosted Windows runner: the real
//! `dante-asio` device process (on the runner's routed IPv4, the stack's
//! test-only system clock: the runner has no dantesync) and an ASIO host on
//! the instance's driver in this process (the probe, as a DAW drives it).
//! RX 1 is subscribed to our own TX 1 through the device's state file, so
//! each impulse the host sends on output 1 goes out through Dante and comes
//! back on input 1. Only our own device is subscribed.
#![cfg(windows)]

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use iem_dante::device::{DeviceConfig, DeviceState, Subscription};

const TEST_DEVICE: &str = include_str!("../../../config/test-dante-device.toml");
/// The instance the test runs (the driver's host tests use 5 to 10).
const INSTANCE: u32 = 13;
/// How long the device may take to publish its shared memory.
const READY: Duration = Duration::from_secs(20);
/// How long the device may take to end after its stop.
const STOP: Duration = Duration::from_secs(15);

/// The test device's config in `dir` at a 10 ms latency (a shared runner
/// schedules a normal-priority thread over 1 ms late now and then: the
/// path is under test here, real-time timing on the PC), with a state file
/// that subscribes RX 1 to our own TX 1.
fn device_in(dir: &Path) -> PathBuf {
    let state = dir.join("state.toml");
    let text = format!(
        "{}\nstate_file = {:?}\n",
        TEST_DEVICE
            .lines()
            .filter(|l| !l.starts_with("state_file"))
            .map(|l| {
                if l.starts_with("latency_ns") {
                    "latency_ns = 10000000"
                } else {
                    l
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        state.display().to_string()
    );
    let path = dir.join("device.toml");
    std::fs::write(&path, &text).unwrap();
    let cfg = DeviceConfig::from_toml(&text).unwrap();
    let mut saved = DeviceState::from_config(&cfg);
    saved.subscriptions = vec![None; cfg.rx.len()];
    saved.subscriptions[0] = Some(Subscription {
        tx_channel: cfg.tx[0].clone(),
        tx_device: cfg.name.clone(),
    });
    saved.save(&state).unwrap();
    path
}

/// The runner's IPv4 address on its default route (connecting a UDP
/// socket picks the route and sends nothing).
fn routed_ipv4() -> Ipv4Addr {
    let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
    probe.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).unwrap();
    match probe.local_addr().unwrap() {
        SocketAddr::V4(a) if !a.ip().is_loopback() && !a.ip().is_unspecified() => *a.ip(),
        other => panic!("no routed IPv4 address on this runner: {other}"),
    }
}

/// The device process; killed if the test ends without stopping it (a
/// failed assertion), so it never outlives the test.
struct Device {
    child: Option<Child>,
    log: PathBuf,
}

impl Device {
    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
            eprintln!("the device's log:\n{}", self.log());
        }
    }
}

fn exe() -> &'static str {
    env!("CARGO_BIN_EXE_dante-asio")
}

#[test]
fn an_impulse_goes_out_through_dante_and_comes_back_to_the_host() {
    let dir = std::env::temp_dir().join(format!("dante-asio-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = device_in(&dir);
    let ip = routed_ipv4();
    let log = dir.join("device.log");
    let child = Command::new(exe())
        .args(["run", "--instance", &INSTANCE.to_string(), "--config"])
        .arg(&config)
        .args(["--ip", &ip.to_string(), "--clock", "system", "--log"])
        .arg(&log)
        .stdin(Stdio::null())
        .spawn()
        .expect("the device process");
    let mut device = Device {
        child: Some(child),
        log,
    };

    // Its shared memory, then a moment for the flow to be granted.
    let t = Instant::now();
    while shm::Shared::open(INSTANCE).is_err() {
        if let Some(status) = device.child.as_mut().unwrap().try_wait().unwrap() {
            panic!(
                "the device ended at the start ({status}):\n{}",
                device.log()
            );
        }
        assert!(
            t.elapsed() < READY,
            "no shared memory after {READY:?}:\n{}",
            device.log()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_secs(2));

    let r = dante_asio::probe::probe(INSTANCE, 6).expect("the probe");
    eprintln!("{r:?}");
    let block = u64::from(r.block);
    assert!(
        r.switches >= 6 * 96_000 / block * 9 / 10,
        "the device drives the host: {} switches in 6 s at block {block}",
        r.switches
    );
    assert!(r.emitted >= 4, "impulses sent: {}", r.emitted);
    assert!(
        r.round_trips.len() as u64 >= r.emitted - 1,
        "impulses back: {} of {}",
        r.round_trips.len(),
        r.emitted
    );
    let (min, max) = (
        *r.round_trips.iter().min().unwrap(),
        *r.round_trips.iter().max().unwrap(),
    );
    assert_eq!(min, max, "one fixed round trip: {:?}", r.round_trips);
    // Out (the driver's output latency) and back in (its input latency):
    // the round trip the driver reports to the DAW is what the path takes.
    assert!(
        min <= (r.latency_in + r.latency_out) as u64 + block,
        "round trip {min} against reported {} + {}",
        r.latency_in,
        r.latency_out
    );
    assert_eq!(r.late, 0, "outputs the host returned too late");

    // A stop as `install.ps1` and the service ask it: the device ends cleanly.
    let status = Command::new(exe())
        .args(["stop", "--instance", &INSTANCE.to_string()])
        .status()
        .unwrap();
    assert!(status.success(), "dante-asio stop: {status}");
    let mut child = device.child.take().unwrap();
    let t = Instant::now();
    let ended = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if t.elapsed() > STOP {
            let _ = child.kill();
            panic!("the device did not end within {STOP:?}:\n{}", device.log());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        ended.success(),
        "the device's exit: {ended}\n{}",
        device.log()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
