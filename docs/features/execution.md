# Child Process Environment Isolation

- Purpose: Document the shared environment sanitizers used when Codemux launches non-interactive child processes.
- Audience: Anyone adding or changing subprocess launch paths.
- Authority: Canonical feature doc for GUI-environment isolation.
- Update when: The sanitized keys, neutralizing overrides, or call sites change.
- Read next: `docs/plans/windows-support.md`, `docs/features/terminal.md`

## Current Model

`src-tauri/src/execution/mod.rs` contains a small cross-platform helper layer. It is not a process sandbox and does not wrap ordinary terminal PTYs.

The helpers remove GUI-session variables such as `DISPLAY`, `WAYLAND_DISPLAY`, `XAUTHORITY`, and desktop-specific socket variables, then add neutralizing values including `BROWSER=true`, `MOZ_NO_REMOTE=1`, `GTK_USE_PORTAL=0`, and `GIO_USE_VFS=local`. The default variants also replace the D-Bus session address with an inert path; `*_keep_dbus` variants preserve D-Bus for commands that need desktop credential or keyring access.

Four entry points cover both Rust process APIs:

- `sanitize_gui_env_std`
- `sanitize_gui_env_tokio`
- `sanitize_gui_env_std_keep_dbus`
- `sanitize_gui_env_tokio_keep_dbus`

## Production Callers

- GitHub authentication and CLI helpers in `src-tauri/src/github.rs`
- OpenCode discovery and server startup in `src-tauri/src/agent_provider/opencode/`
- file and external-tool commands in `src-tauri/src/commands/`

Interactive terminal sessions deliberately inherit the user's normal desktop environment. The sanitizers are aimed at backend-launched helpers that should not accidentally open a browser, portal, or desktop window.

## Security Boundary

Environment sanitization prevents common accidental GUI launches; it is not containment. A child can still access the filesystem, network, and desktop sockets it can discover by other means. Codemux currently has no generic Bubblewrap, AppContainer, `sandbox-exec`, virtual-display, or per-workspace execution-policy layer.

Any future sandbox should be introduced as a new, independently reviewed boundary rather than inferred from these helpers.

## Verification

- unit tests in `src-tauri/src/execution/mod.rs` pin the key and override sets
- `src-tauri/tests/gui_leak_prevention.rs` exercises both standard and Tokio command builders
- `cargo test --manifest-path src-tauri/Cargo.toml` covers the module and integration tests
