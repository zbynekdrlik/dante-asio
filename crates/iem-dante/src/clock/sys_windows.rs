//! The media clock's OS clocks on Windows. dantesync steers the system
//! time's rate (`SetSystemTimeAdjustmentPrecise`) and steps it only at a
//! date step (`NtSetSystemTime`); QPC is the raw counter and is not steered.
//! So both the wall and the "monotonic" clock read the precise system time
//! (`GetSystemTimePreciseAsFileTime`): k = (wall − D) − wall = −D, steady
//! between date steps; a date step changes k, which the tracker takes as a
//! re-anchor after its confirmations (dantesync's daily step, 02:00 UTC).

use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows_sys::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, INFINITE, SetWaitableTimer,
    TIMER_ALL_ACCESS, WaitForSingleObject,
};

/// 100 ns intervals from 1601-01-01 to 1970-01-01.
const UNIX_EPOCH_FT: i64 = 116_444_736_000_000_000;
/// Waits up to this long are spun (yielding), never slept: on the FOH PC
/// the media threads, alone on their CPUs at priority 31, still woke past
/// 2.7 ms now and then while the CPUs themselves were never held over
/// 0.2 ms (`dante-asio jitter`): the waitable timer's late wakes. Longer
/// waits (the clock not ready) sleep on the timer, the last 50 ms spun.
const SPIN_NS: i64 = 50_000_000;

/// The precise system time, ns since 1970.
pub fn realtime_ns() -> i64 {
    let mut ft = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    // SAFETY: `ft` is a valid, writable FILETIME.
    unsafe { GetSystemTimePreciseAsFileTime(&mut ft) };
    let t = (i64::from(ft.dwHighDateTime) << 32) | i64::from(ft.dwLowDateTime);
    (t - UNIX_EPOCH_FT) * 100
}

/// The clock k is taken against: the precise system time too (see the module).
pub fn monotonic_ns() -> i64 {
    realtime_ns()
}

/// This thread's high-resolution waitable timer (made once per thread).
fn timer() -> HANDLE {
    thread_local! {
        static TIMER: OnceLock<Timer> = const { OnceLock::new() };
    }
    TIMER.with(|t| {
        t.get_or_init(|| {
            // SAFETY: no name, no security attributes; the handle is closed on
            // the thread's end.
            let h = unsafe {
                CreateWaitableTimerExW(
                    std::ptr::null(),
                    std::ptr::null(),
                    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                    TIMER_ALL_ACCESS,
                )
            };
            Timer(h)
        })
        .0
    })
}

struct Timer(HANDLE);

impl Drop for Timer {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle is this thread's timer, closed once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// Sleeps until [`monotonic_ns`] reaches `target`: the timer for all but
/// the last [`SPIN_NS`], then a spin.
pub fn sleep_until_monotonic(target: i64) {
    let left = target - monotonic_ns();
    if left > SPIN_NS {
        let h = timer();
        if !h.is_null() {
            // Relative due time: negative, in 100 ns units.
            let due = -((left - SPIN_NS) / 100);
            // SAFETY: a valid timer handle, a valid due time, no callback.
            let set = unsafe { SetWaitableTimer(h, &due, 0, None, std::ptr::null(), 0) };
            if set != 0 {
                // SAFETY: a valid handle.
                unsafe { WaitForSingleObject(h, INFINITE) };
            }
        } else {
            std::thread::sleep(std::time::Duration::from_nanos((left - SPIN_NS) as u64));
        }
    }
    // The spin yields: the media threads share cores on Windows (the FOH
    // PC gives the device two), and a plain spin kept the RX thread off its
    // core (RX packets up to 48 ms late, 2026-10-29).
    while monotonic_ns() < target {
        std::thread::yield_now();
    }
}
