# dante-asio

A Windows ASIO driver whose audio goes straight to our own Dante device (one device process per instance).
Design: `docs/design/2026-10-29-dante-asio-design.md`; MVP plan: `docs/plans/2026-10-29-dante-asio-mvp-plan.md`.

- Branches: `dev` and `main` only; PRs dev → main; merge commits.
- No local cargo: GitHub CI (`ubuntu-latest`, `windows-latest`) builds and tests.
- Version: `[workspace.package].version`; dev > main on PRs (`scripts/check_version.py`).
- No site data in the repo: addresses, real device and channel names live only in the PC's config.
- Dante: never change another device's subscriptions; tests subscribe our own device to itself.
- `crates/iem-dante` (and `crates/audio-io`, which it uses) is frozen: dante-stack owns all Dante code (zbynekdrlik/dante-stack#1). A Dante protocol bug goes there as a ticket; this repo only moves the stack version. The copy goes once `iem-dante = { package = "dante-stack", git = "https://github.com/zbynekdrlik/dante-stack", tag = "v0.1.0" }` builds green.
- dantesync is the PC's only clock owner; the device never opens UDP 319/320.
- Every message to the owner in Slovak; code, commits and docs in English.
