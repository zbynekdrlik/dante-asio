//! `dante-asio run` on Windows: the instance's Dante device on its own
//! address, its pump on the bridge, the shared memory for the driver, and a
//! heartbeat in the shared header until Ctrl+C or the device ends.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use iem_dante::daemon::{Options, Place};
use iem_dante::host::{self, Signals, Stopped};
use iem_dante::media::source::Source;
use iem_dante::stream::{self, Map, Pump, PumpStats};
use shm::{Shape, Shared};
use tracing::{info, warn};

use crate::bridge::{Bridge, SharedLink};

/// The DAW keeps this much of W for the copy back (µs).
const COPY_MARGIN_US: u64 = 40;

/// Runs instance `instance` from the device config at `config` on `ip`.
pub fn run(instance: u32, config: PathBuf, ip: std::net::Ipv4Addr) -> Result<(), String> {
    let cfg = host::read_config(&config).map_err(|e| e.to_string())?;
    let shape = Shape::new(cfg.rx.len() as u32, cfg.tx.len() as u32, cfg.block as u32)
        .ok_or("the channels (at most 64 each way) or the block do not fit the driver")?;
    let opts = Options {
        config,
        place: Place::Ip(ip),
        control: PathBuf::new(),
    };
    let engine_tx: Vec<usize> = (0..cfg.tx.len()).collect();
    let prepared = host::prepare(&opts, &engine_tx).map_err(|e| e.to_string())?;
    let cfg = prepared.config().clone();
    let media = prepared.media();
    for &tx in &engine_tx {
        media.tx.set_source(tx, Source::Engine);
    }
    let timing = crate::timing(&cfg);
    let latency = crate::latencies(&cfg);
    let shared = Arc::new(
        Shared::create(instance, shape, cfg.sample_rate, latency)
            .map_err(|e| format!("the shared memory: {e}"))?,
    );
    let stats = Arc::new(PumpStats::new(timing, cfg.sample_rate));
    media.attach_engine(Arc::clone(&stats));
    let margin_us = timing.margin * 1_000_000 / u64::from(cfg.sample_rate);
    let wait = Duration::from_micros(margin_us.saturating_sub(COPY_MARGIN_US).max(1));
    let bridge = Bridge::new(SharedLink(Arc::clone(&shared)), wait);
    let map = Map {
        rx: (0..cfg.rx.len()).collect(),
        tx: engine_tx,
    };
    let pump = Pump::new(
        bridge,
        map,
        timing,
        cfg.encoding,
        Arc::clone(&media.rx),
        Arc::clone(&media.tx),
        Arc::clone(&stats),
    );
    let stop = Arc::new(AtomicBool::new(false));
    let pump = stream::spawn(Arc::clone(&media.clock), pump, Arc::clone(&stop))
        .map_err(|e| format!("the pump: {e}"))?;
    let asked = Arc::new(AtomicBool::new(false));
    let device = prepared
        .start(Signals::Report(Arc::clone(&asked)))
        .map_err(|e| e.to_string())?;
    info!(
        instance,
        rx = shape.rx,
        tx = shape.tx,
        block = shape.block,
        latency_in = latency.0,
        latency_out = latency.1,
        "dante-asio instance serving"
    );
    let start = Instant::now();
    let why = loop {
        shared
            .header()
            .heartbeat_ms
            .store(start.elapsed().as_millis() as u64, Ordering::Release);
        if asked.load(Ordering::Acquire) {
            break None;
        }
        if let Some(e) = device.ended() {
            break Some(e);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    stop.store(true, Ordering::Release);
    let _ = pump.join();
    match device.stop(Duration::from_secs(7)) {
        Stopped::Done => {}
        other => warn!(?other, "the device's stop"),
    }
    match why {
        None => Ok(()),
        Some(e) => Err(e),
    }
}
