//! The pump's processor: each block's inputs go to the DAW through the
//! shared memory and its outputs come back within the work margin W.
//!
//! Per block: the RX samples into half `seq % 2`'s input buffers (24 bits
//! left-justified in `i32`, as ASIO's `Int32LSB`), `seq` published and the
//! block event signalled; then the DAW's `ack` of that `seq` waited for (a
//! spin, at most the deadline: W less a margin for the copy back); its
//! output buffers into the block. No DAW attached, or no `ack` in time: the
//! outputs stay silent and a late one counts in `late`.

use std::time::{Duration, Instant};

use audio_io::{Block, Process};

/// The DAW side of the shared memory, as the bridge uses it (the mapping on
/// Windows, a stand-in in the tests).
pub trait Link: Send {
    /// A DAW holds the buffers.
    fn attached(&self) -> bool;
    /// The `ch`th input buffer of `half`.
    fn input(&mut self, half: u32, ch: usize) -> &mut [i32];
    /// The `ch`th output buffer of `half` into `dst` (f64, ±1 full scale).
    fn read_output(&self, half: u32, ch: usize, dst: &mut [f64]);
    /// Publishes block `seq` (its inputs are in) and wakes the driver.
    fn publish(&self, seq: u64);
    /// The newest block the DAW returned.
    fn ack(&self) -> u64;
    /// One block that came too late.
    fn count_late(&self);
}

/// f64 (±1 full scale) to 24 bits left-justified, as the pump reads them.
pub fn to_i32(x: f64) -> i32 {
    (x * 2_147_483_648.0)
        .round()
        .clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

/// 24 bits left-justified to f64 (±1 full scale), exact.
pub fn to_f64(s: i32) -> f64 {
    f64::from(s) / 2_147_483_648.0
}

/// The pump's processor for one instance.
pub struct Bridge<L: Link> {
    link: L,
    seq: u64,
    /// How long the DAW may take to return a block.
    wait: Duration,
}

impl<L: Link> Bridge<L> {
    /// `wait`: the DAW's time for a block (W less the copy's margin).
    pub fn new(link: L, wait: Duration) -> Self {
        Bridge { link, seq: 0, wait }
    }
}

impl<L: Link> Process for Bridge<L> {
    fn process(&mut self, block: &mut Block<'_>) {
        if !self.link.attached() {
            return;
        }
        self.seq += 1;
        let half = (self.seq % 2) as u32;
        for ch in 0..block.inputs() {
            let src = block.input(ch);
            for (d, &x) in self.link.input(half, ch).iter_mut().zip(src) {
                *d = to_i32(x);
            }
        }
        self.link.publish(self.seq);
        let deadline = Instant::now() + self.wait;
        while self.link.ack() < self.seq {
            if Instant::now() >= deadline {
                self.link.count_late();
                return;
            }
            std::hint::spin_loop();
        }
        for ch in 0..block.outputs() {
            self.link.read_output(half, ch, block.output(ch));
        }
    }
}

#[cfg(windows)]
pub use win::SharedLink;

#[cfg(windows)]
mod win {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use shm::Shared;

    use super::Link;

    /// The instance's mapping as the bridge's link.
    pub struct SharedLink(pub Arc<Shared>);

    impl Link for SharedLink {
        fn attached(&self) -> bool {
            self.0.header().host_attached.load(Ordering::Acquire) != 0
        }

        fn input(&mut self, half: u32, ch: usize) -> &mut [i32] {
            let n = self.0.shape().block as usize;
            let p = self
                .0
                .buffer(half, true, ch as u32)
                .expect("a channel of the shape");
            // SAFETY: `n` samples inside the mapping; this half is the
            // device's until it publishes it.
            unsafe { std::slice::from_raw_parts_mut(p, n) }
        }

        fn read_output(&self, half: u32, ch: usize, dst: &mut [f64]) {
            let n = self.0.shape().block as usize;
            let p = self
                .0
                .buffer(half, false, ch as u32)
                .expect("a channel of the shape");
            // SAFETY: `n` samples inside the mapping; the DAW returned them.
            let src = unsafe { std::slice::from_raw_parts(p, n) };
            for (y, &s) in dst.iter_mut().zip(src) {
                *y = super::to_f64(s);
            }
        }

        fn publish(&self, seq: u64) {
            self.0.header().seq.store(seq, Ordering::Release);
            self.0.signal_block();
        }

        fn ack(&self) -> u64 {
            self.0.header().ack.load(Ordering::Acquire)
        }

        fn count_late(&self) {
            self.0.header().late.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests;
