# Verification artifacts

The `core-*` and `settings-*` PNGs below use the repository's synthetic Tauri
mock. They establish visual layout only, not native plugin operation.

- `core-before.png`: baseline workspace without the plugin platform.
- `core-after.png`: implementation workspace with zero installed add-ons.
- `settings-after.png`: permanent Add-ons Settings, default empty state.
- `settings-small-dark.png`, `settings-small-light.png`: 800×600 Settings.
- `settings-chat-gui-off.png`: Add-ons Settings remains available with chat GUI off.

`host-timing-linux.json` contains standalone host timing samples.
`packaged-windows.json` records installed NSIS payload checks from `5a643feb`.
`packaged-linux.json` records deb/AppImage payload checks from `5a643feb`.
These JSON files cover runtime containment, not stock desktop GUI behavior.

Native and packaged verification is tracked separately in ../IMPLEMENTATION.md.

`ui-validation-linux.json` compares native validation before and after incremental
content validation. Reproduce the workload with
`cargo run --release -j 2 --manifest-path src-tauri/addon-protocol/Cargo.toml --example ui-validation-benchmark`.
It excludes IPC and desktop rendering.

Files named `native-*.png` capture the actual stock installed app in disposable
GitHub runners using synthetic account/project data. `native-ui-*.json` records
installer SHA-256/build revision, harness revision, hardware, completed checks
and failures separately. A `status: failed` run is never an overall passing gate;
its individual completed checks can supplement another run on the same exact
installer. Provider CLIs are intentionally absent from these runners, so core
chat checks cover controlled typing and no submission, not provider inference.
The implementation ledger links each run and explains fixture seams.
