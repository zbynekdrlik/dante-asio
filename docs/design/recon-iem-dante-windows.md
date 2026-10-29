# Recon: iem-dante on Windows (2026-10-29, iemmixer at a510531)

- Mostly portable; host.rs is cfg(linux)-gated as a whole: port it (UnixListener control socket -> named pipe or drop; unix signals -> own IPC stop; /sys,/proc NIC info -> GetAdaptersAddresses).
- Linux-only: clock.rs `mod linux` (CLOCK_MONOTONIC/REALTIME, clock_nanosleep), tx.rs ask_realtime (SCHED_FIFO) and place (sched_setaffinity), ctl.rs unix socket, health.rs (box-only, do not copy), libc dep.
  Windows: MMCSS "Pro Audio" + THREAD_PRIORITY_TIME_CRITICAL; SetThreadSelectedCpuSets; high-res waitable timer + short spin; windows-sys.
- Pitfalls: (1) UDP WSAECONNRESET after ICMP unreachable kills recv_from loops (arc.rs:393, flows/server.rs:398, cmc.rs:79, info.rs:669; rx/tx flow sockets) -> disable SIO_UDP_CONNRESET on every socket. (2) info multicasts rely on Linux source-address routing -> set IP_MULTICAST_IF explicitly.
- Clock: DSYX UDP 127.0.0.1:31900; portable parsing/Tracker/PtpClock trait; implement WinClock. Risk: QPC is likely not rate-steered by dantesync's SetSystemTimeAdjustmentPrecise -> use GetSystemTimePreciseAsFileTime - D, or estimate the QPC-to-wall rate. Verify dantesync on Windows serves the date-offset extension (ptp_phase_lock).
- Host API: host::prepare(opts, engine_tx) -> Prepared{media(), config()}; tx.set_source(i, Source::Engine); PumpStats; Timing{block, playout, margin}; Pump::new(processor: impl iem_audio_io::Process, Map, timing, bits, rx, tx, stats); stream::spawn(clock, pump, stop); prepared.start(Signals::Report); Host::stop.
  Process trait: process(&mut self, block: &mut Block) (channel-major f64), discontinuity(). The ASIO hand-off implements Process.
- Ports bound to the device IP: UDP 4440 ARC, 8800 CMC, 8700 info, 4455 flow control; flows ephemeral; mDNS per-address daemon (mdns-sd, shares 5353 with Bonjour - test). Two instances need two IPs. device_id from IP + PROCESS_ID 0 -> distinct per IP.
- Deps all build on Windows (if-addrs, mdns-sd, mio, socket2, tokio) except libc. iem-audio-io pulls azo/iem-win on Windows; only Process/Block/StreamStats/hist/period_ns used.
- License: iem-dante is GPL-3.0-or-later (inferno-derived); dante-asio becomes GPL too.
