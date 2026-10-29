# Recon: an ASIO driver in Rust (2026-10-29)

- IASIO vtable after IUnknown, 21 methods: init, getDriverName, getDriverVersion, getErrorMessage, start, stop, getChannels, getLatencies, getBufferSize, canSampleRate, getSampleRate, setSampleRate, getClockSources, setClockSource, getSamplePosition, getChannelInfo, createBuffers, disposeBuffers, controlPanel, future, outputReady. x64: extern "system", this first. #pragma pack(4); ASIOSamples/TimeStamp = {u32 hi, lo}; long = i32; sample rate double by value.
- Callbacks: bufferSwitch, sampleRateDidChange, asioMessage, bufferSwitchTimeInfo; kAsioSupportsTimeInfo; kAsioResetRequest for device-side changes; future() success = ASE_SUCCESS 0x3f4847a0.
- Sample types: Int32LSB=18 (bit-exact 24-in-32), Float32LSB=19.
- outputReady: return ASE_OK to let host signal early (saves a block); must work without it.
- Registry: HKCR\CLSID\{guid} + InprocServer32 (DLL path, ThreadingModel=Apartment); HKLM\SOFTWARE\ASIO\<Name> CLSID, Description. Host passes CLSID as IID: QueryInterface must accept its own CLSID. One DLL, N CLSIDs = N instances.
- References: transit-audio/transit crates/audio-asio/src/abi.rs (Apache-2.0, hand-written ABI, layout-checked); diegobragaferreira/irig-asio-driver (MIT, Rust driver skeleton); dechamps/FlexASIO (MIT, edge cases) + ASIOTest (conformance tool); aura-engine (GPL/noncommercial, design only); SynchronousAudioRouter SarAsio (shm+events pattern).
- SDK 2.3.4 dual-licensed incl. GPLv3. dante-asio is GPL anyway (iem-dante GPL-3.0+), so SDK headers usable under GPLv3; hand-written ABI still simplest.
- Hand-off: device process is the clock; driver thread waits on a named auto-reset event per block, calls bufferSwitchTimeInfo; zero-copy double buffers in the shared mapping; heartbeat; free-run silence on high-res waitable timer if device gone, kAsioResetRequest on return. MMCSS "Pro Audio" + AVRT_PRIORITY_CRITICAL both sides; NetworkThrottlingIndex 0xFFFFFFFF.
- UDP: Winsock first; RIO polled later if jitter demands; skip XDP on Win10 LTSC.
- Tray: tray-icon 0.26 + muda, plain Win32 message loop.
- Unknown: Ableton 12's use of outputReady / bufferSwitchTimeInfo -> log in the driver.
