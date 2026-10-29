# Recon: dantesync on Windows (dantesync HEAD ab3b870, v1.20.0), 2026-10-29

- Discipline: SetSystemTimeAdjustmentPrecise (rate), NtSetSystemTime (steps, only D steps). wall = PTP + D; D steps daily at 02:00 UTC (whole 200 ms), announced >= 5 s ahead.
- QPC is NOT steered. Rate-correct: GetSystemTimePreciseAsFileTime (jumps only at announced D steps). dantesync's own monotonic equivalent: QPC * inc/adj (piecewise).
  => WinClock: PTP = precise wall - D (D from DSYX), with STEP_GUARD and slew guards; or QPC + k re-estimated per poll.
- Time server UDP 31900 on 0.0.0.0, Windows too; DSYX request zero-padded to 64 B; reply 112 B; receive buffer >= 256 B (WSAEMSGSIZE). Also HTTP /status :8898, pipe \\.\pipe\dantesync.
- Incident: frequent micro D steps starved DVS's ASIO input (clock-discipline-and-testing.md:520-525) -> daily mode only.
- dantesync RT on Windows: REALTIME_PRIORITY_CLASS, timeBeginPeriod(1), Npcap HostHighPrec PTP capture, SIO_UDP_CONNRESET off. No MMCSS, no waitable timers, no affinity.
- dante-ndi: empty directory, nothing there. No shared code among repos; iem-dante's dsyx.rs reimplements dantesync's protocol. dantesync MIT, iem-dante GPL-3.0+.
