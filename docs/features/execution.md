# Child Process Environment Isolation

- Purpose: Document the shared environment sanitizers used when Codemux launches non-interactive child processes.
- Audience: Anyone adding or changing subprocess launch paths.
- Authority: Canonical feature doc for GUI-environment isolation.
- Update when: The sanitized keys, neutralizing overrides, or call sites change.
- Read next: `docs/plans/windows-support.md`, `docs/features/terminal.md`

This doc covers two independent layers in `src-tauri/src/execution/mod.rs`:
GUI-session hygiene (below) and AppImage hygiene (see "AppImage Environment").

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

## AppImage Environment

This layer is separate from GUI hygiene and applies to *interactive* terminals too.

When Codemux runs from an AppImage, `AppRun` rewrites loader and toolkit variables to point into the mounted AppDir (`/tmp/.mount_codemuXXXXXX`) so the bundled binary finds its bundled libraries. Those values are correct for Codemux and wrong for every child: a shell that inherits `LD_LIBRARY_PATH` resolves system binaries against our bundled `libssl`/`libpcre2`. On a host whose libraries are newer than the bundle this fails before `main` — e.g. `cargo` dying with ``version `OPENSSL_3.5.0' not found`` without ever reading `Cargo.toml`. `PYTHONHOME` and `PATH` break system `python3` and shadow the user's toolchain the same way.

Four rule classes, all keyed off `APPDIR`:

- **Path lists** (`PATH`, `LD_LIBRARY_PATH`, `PYTHONPATH`, `XDG_DATA_DIRS`, …) — AppRun *prepends*, so only AppDir entries are dropped and the user's entries survive. A list left with nothing is removed entirely rather than set to empty. Empty entries (an empty field means "the current directory", and AppRun's trailing `:` leaves one) are dropped uniformly whenever the list is rewritten, so the rule never depends on whether an AppDir entry happened to be present. `LD_PRELOAD` is deliberately *not* in this class: it is space-or-colon separated and no AppRun variant sets it, so any value seen is the user's own.
- **AppDir-rooted scalars** (`PYTHONHOME`, `GTK_EXE_PREFIX`, `GDK_PIXBUF_MODULE_FILE`, …) — removed only when the value actually points into the AppDir, so a user's own value survives.
- **Launch markers** (`APPIMAGE`, `APPIMAGE_UUID`, `ARGV0`, `OWD`) and **AppRun-overwritten values** (`GTK_THEME`, `APPIMAGE_GTK_THEME`) — not AppDir-rooted, but nothing of the user's is left in them either, so they are removed unconditionally when running from a bundle. The GTK theme pair otherwise pins every GTK program started from a terminal to `Adwaita:dark` regardless of the desktop theme.
- **Fixed AppRun literals** (`GDK_BACKEND=x11`, `PYTHONDONTWRITEBYTECODE=1`) — removed only when the value still matches what AppRun sets. `GDK_BACKEND=x11` (the plugin's Wayland-crash workaround) otherwise drags every GTK child through XWayland; `GDK_BACKEND` is also in `gui_env_keys()`, which covers backend helpers, and this class is what closes the same hole for interactive terminals.

Entry points, mirroring the GUI helpers:

- `sanitize_appimage_env_pty` (`portable_pty::CommandBuilder`)
- `sanitize_appimage_env_std`
- `sanitize_appimage_env_tokio`
- `sanitized_child_path` — `PATH` minus AppDir entries, for call sites that *build* a child `PATH`
- `host_command` / `host_command_tokio` — **the default for shelling out to a host binary**

Prefer `host_command("git")` over `std::process::Command::new("git")` anywhere Codemux runs a program belonging to the user's system rather than to our bundle. It is a constructor so it drops into builder chains, and later explicit `.env()` calls still win. `git` is the concrete motivator: under an AppImage a raw `Command::new("git")` prints ``libpcre2-8.so.0: no version information available`` and fails outright once the host/bundle version skew is large enough.

`host_command` is orthogonal to the GUI sanitizers — it touches only loader/toolkit path variables and never `DISPLAY`/`WAYLAND_DISPLAY`. That is what makes it safe at sites that must *keep* the desktop environment, such as `open_in_editor`, which deliberately skips `sanitize_gui_env_std` so the editor can actually show a window.

Three invariants that are easy to break:

1. **Leaf spawns only.** The pty-daemon is the Codemux binary re-executed and still needs the AppDir libraries itself, so the strip must never move up to `pty_daemon/supervisor.rs` — doing so stops the daemon from launching.
2. **Sanitize before layering `PATH`.** These helpers derive `PATH` from the *process* environment. Any call site that then sets its own `PATH` (CLI shim injection) must run after the sanitizer and start from `sanitized_child_path()`, or it re-introduces the AppDir entries. Every call site applies the sanitizer first and lets its own explicit pairs win.
3. **Nothing in-tree may read `$APPDIR`/`$APPIMAGE` to learn how it was installed.** Those variables are gone in any process Codemux spawned, so a `codemux` CLI run from a terminal pane or a setup script sees "not an AppImage". `web_remote/connect.rs` is the live example: it needs to know the install shape to pick a `systemd` `ExecStart`, and derives the AppDir from `current_exe()`'s layout (an `AppRun` three levels up) instead, refusing any candidate under a throwaway `/tmp/.mount_*` mount.

Call sites:

- PTY and agent paths — `terminal/mod.rs` (in-process PTY), `pty_daemon/server.rs` (daemon-side child), `remote/pty.rs` (headless PTY), `json_rpc_child/mod.rs` (all agent CLIs and sidecars)
- host-binary paths via `host_command` — `scripts.rs` (workspace setup/teardown `sh -c`), `ai.rs`, `github.rs`, `automations/executor.rs`, `commands/workspace.rs` (editor launch, `hyprctl`)

Known remaining gap: this is not yet applied to every `Command::new` in the codebase (`agent_browser.rs`, `mcp/runtime.rs`, `commands/files.rs`, the agent-provider probes, and others still spawn raw). Those are lower-impact but the same bug class — extend `host_command` to them when touched.

Everything is gated on `APPDIR`, which only AppRun sets, so distro packages (AUR, `.deb`, `.rpm`) are unaffected: the fixup list is empty and every call site is a no-op.

## Security Boundary

Environment sanitization prevents common accidental GUI launches; it is not containment. A child can still access the filesystem, network, and desktop sockets it can discover by other means. Codemux currently has no generic Bubblewrap, AppContainer, `sandbox-exec`, virtual-display, or per-workspace execution-policy layer.

Any future sandbox should be introduced as a new, independently reviewed boundary rather than inferred from these helpers.

## Verification

- unit tests in `src-tauri/src/execution/mod.rs` pin the key and override sets, and cover the AppImage rules against a simulated environment (pure functions, so no process-env mutation)
- `src-tauri/tests/gui_leak_prevention.rs` exercises both standard and Tokio command builders
- `src-tauri/tests/appimage_env_hygiene.rs` re-executes the test binary under a simulated AppImage environment and asserts a real PTY child and a real Tokio child come out clean; it also pins the non-AppImage no-op
- `cargo test --manifest-path src-tauri/Cargo.toml` covers the module and integration tests

The AppImage sanitizers only act when `APPDIR` is set, so a test that needs them active cannot simply call them — the test process must itself have `APPDIR`. Mutating the current process's environment is racy under parallel tests and `unsafe`, hence the re-exec pattern in `appimage_env_hygiene.rs`. Every assertion about the sanitizers must go through `run_inner_test`; run outside it, the fixup list is empty and the assertions pass vacuously. The harness asserts the child printed `test result: ok. 1 passed` (the full summary line, since a bare `1 passed` also matches `11 passed`), because a libtest filter matching nothing still exits 0 and would otherwise make the test green for the wrong reason.
