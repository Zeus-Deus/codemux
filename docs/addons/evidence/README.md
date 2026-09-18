# Verification artifacts

All PNG images use the repository's synthetic Tauri mock. They establish visual
layout only, not native plugin operation.

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
