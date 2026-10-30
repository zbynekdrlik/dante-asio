//! What the driver knows about its device, from the shared header alone: a
//! heartbeat that moves, the device's pid, the shape it published. A pid
//! check of the process itself is not usable (a DAW may run as another,
//! non-elevated user than the device). Portable, so it is tested on every
//! OS.

use std::time::{Duration, Instant};

use shm::Shape;

/// A heartbeat unchanged this long: the device is stale (it writes one
/// every 100 ms).
pub const STALE: Duration = Duration::from_millis(500);

/// The device's state, as the header shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Live,
    /// The heartbeat stopped moving.
    Stale,
    /// Another device process took over the same mapping.
    Restarted,
    /// The header's shape is not the one the DAW opened: the DAW must reset.
    Reshaped,
}

/// What one look at the header saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    pub beat: u64,
    pub pid: u32,
    /// `None` when the header holds no valid shape.
    pub shape: Option<Shape>,
}

/// The device as last seen.
#[derive(Debug)]
pub struct Watch {
    opened: Shape,
    pid: u32,
    beat: u64,
    beat_seen: Instant,
}

impl Watch {
    /// A watch for a DAW that opened `opened`, starting from `first`.
    pub fn new(opened: Shape, first: Facts, now: Instant) -> Watch {
        Watch {
            opened,
            pid: first.pid,
            beat: first.beat,
            beat_seen: now,
        }
    }

    /// The verdict on `facts` seen at `now`.
    pub fn observe(&mut self, facts: Facts, now: Instant) -> Verdict {
        if facts.shape != Some(self.opened) {
            return Verdict::Reshaped;
        }
        if facts.pid != self.pid {
            self.pid = facts.pid;
            self.beat = facts.beat;
            self.beat_seen = now;
            return Verdict::Restarted;
        }
        if facts.beat != self.beat {
            self.beat = facts.beat;
            self.beat_seen = now;
            return Verdict::Live;
        }
        if now.duration_since(self.beat_seen) >= STALE {
            Verdict::Stale
        } else {
            Verdict::Live
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape() -> Shape {
        Shape::new(2, 2, 32).unwrap()
    }

    fn facts(beat: u64, pid: u32) -> Facts {
        Facts {
            beat,
            pid,
            shape: Some(shape()),
        }
    }

    #[test]
    fn a_moving_heartbeat_is_live() {
        let t = Instant::now();
        let mut w = Watch::new(shape(), facts(0, 7), t);
        assert_eq!(w.observe(facts(100, 7), t + STALE), Verdict::Live);
        assert_eq!(w.observe(facts(200, 7), t + 2 * STALE), Verdict::Live);
    }

    #[test]
    fn a_heartbeat_unchanged_for_stale_is_stale_and_live_again_once_it_moves() {
        let t = Instant::now();
        let mut w = Watch::new(shape(), facts(5, 7), t);
        let just_under = STALE - Duration::from_millis(1);
        assert_eq!(w.observe(facts(5, 7), t + just_under), Verdict::Live);
        assert_eq!(w.observe(facts(5, 7), t + STALE), Verdict::Stale);
        assert_eq!(w.observe(facts(6, 7), t + 2 * STALE), Verdict::Live);
        assert_eq!(
            w.observe(facts(6, 7), t + 2 * STALE + just_under),
            Verdict::Live
        );
    }

    #[test]
    fn another_pid_is_a_restart_once_then_live() {
        let t = Instant::now();
        let mut w = Watch::new(shape(), facts(900, 7), t);
        assert_eq!(w.observe(facts(3, 8), t), Verdict::Restarted);
        assert_eq!(w.observe(facts(4, 8), t), Verdict::Live);
        // The restart's beat is the new baseline: unchanged from it is stale.
        assert_eq!(w.observe(facts(4, 8), t + STALE), Verdict::Stale);
    }

    #[test]
    fn another_shape_or_none_is_reshaped_before_anything_else() {
        let t = Instant::now();
        let mut w = Watch::new(shape(), facts(0, 7), t);
        let other = Facts {
            shape: Shape::new(2, 2, 64),
            ..facts(0, 8)
        };
        assert_eq!(w.observe(other, t), Verdict::Reshaped);
        let none = Facts {
            shape: None,
            ..facts(1, 7)
        };
        assert_eq!(w.observe(none, t), Verdict::Reshaped);
        // Neither moved the baseline: the same pid is no restart.
        assert_eq!(w.observe(facts(1, 7), t), Verdict::Live);
    }
}
