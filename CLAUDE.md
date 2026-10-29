# dante-asio

A Windows ASIO driver whose audio goes straight to our own Dante device (one device process per instance).
Design: `docs/design/2026-10-29-dante-asio-design.md`; MVP plan: `docs/plans/2026-10-29-dante-asio-mvp-plan.md`.

- Branches: `dev` and `main` only; PRs dev → main; merge commits.
- No local cargo: GitHub CI (`ubuntu-latest`, `windows-latest`) builds and tests.
- Version: `[workspace.package].version`; dev > main on PRs (`scripts/check_version.py`).
- No site data in the repo: addresses, real device and channel names live only in the PC's config.
- Dante: never change another device's subscriptions; tests subscribe our own device to itself.
- dantesync is the PC's only clock owner; the device never opens UDP 319/320.
- Every message to the owner in Slovak; code, commits and docs in English.
