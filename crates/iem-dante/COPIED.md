# Copied from iemmixer

`crates/iem-dante` and `config/test-dante-device.toml` are copies of
[iemmixer](https://github.com/zbynekdrlik/iemmixer) at commit `a510531`
(2026-10-29). `crates/audio-io` holds the parts of iemmixer's `iem-audio-io`
that `iem-dante` uses: `Process`, `Block`, `StreamStats`, `Planar`, `Fault`,
`panic_message` (`lib.rs`), `hist.rs` and `telemetry.rs` with its tests; the
ASIO host and the other backends stay in iemmixer.

Changes here: the `iem_audio_io` paths read `audio_io`; the Windows port
(plan Task 2 onward). The copies may drift until our Dante applications share
one stack (owner, 2026-10-29, after the MVP).
