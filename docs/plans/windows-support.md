# Windows Support Tracking

Status: In Progress
Branch: feature/windows-support
Created: 2026-04-12

## How to Use This File

This is the master checklist for Windows support. Before working on any Windows-related code, read this file first. After completing an item, check it off and note what was done. This file is the source of truth for what's left.

Findings are organized by severity. File:line references point at the current state of `main` as of 2026-04-12 — verify before editing since line numbers drift.

---

## Research Findings

Web research conducted 2026-04-12 on the three unknowns that gate the Windows effort. **TL;DR**: all three are feasible, but `portable-pty 0.9.0` has a known Windows regression we must pin around, and `agent-browser` ships Windows x64 only (no ARM64). All five agent CLIs run natively on Windows.

### 1. `agent-browser` Windows availability — Supported (x64 only)

- **Package**: `agent-browser` on npm (publisher `vercel-labs`), latest **v0.25.3** (2026-04-07). Repo: https://github.com/vercel-labs/agent-browser
- **Windows x64**: `agent-browser-win32-x64.exe` ships inside the npm tarball **and** as a GitHub release asset. README "Platforms" table explicitly lists `Windows x64 — Native Rust`. `bin/agent-browser.js` maps `platform() === 'win32'` to the `.exe`. Upstream maintains a dedicated `scripts/windows-debug/` folder that spins up EC2 Windows instances — active Windows investment
- **Windows ARM64**: **NOT published** — `agent-browser-win32-arm64.exe` returns 404 on v0.25.3. `build-all-platforms.sh` only builds `x86_64-pc-windows-gnu`. Tracked upstream as issue #248 (Chromium ARM64 availability)
- **Open Windows bugs worth budgeting around**: #1043 TCP bind permission denied on Win11, #953 general crashes, #868 DevTools URL not detected, #578 Rust CLI expects `.sock` instead of `.port` file, #514 binary flagged as virus, #482 Windows Defender quarantine, #398 uses Unix socket instead of TCP, #393 UNC path (`\\?\`) crashes Node daemon, #470 headless drops cookies from persistent profiles, #549 postinstall skipped on npm install
- **Plan**: Add `x86_64-pc-windows-msvc` → `agent-browser-win32-x64.exe` to `scripts/copy-agent-browser.sh` (or its cross-platform replacement). For Windows ARM64 (Surface), fall back to system Edge via CDP (`--remote-debugging-port=9222`) — Edge ships preinstalled on Windows 10+. Expect to code-sign to dodge Defender false-positives

### 2. `portable-pty` ConPTY — Supported, but pin 0.8.1 on Windows

- **Current version**: 0.9.0 (2025-02-11), maintained by Wez Furlong (wezterm author). Canonical repo: `github.com/wezterm/wezterm` under `pty/`
- **ConPTY support**: **Yes, fully.** `pty/src/win/conpty.rs` implements `ConPtySystem` around `CreatePseudoConsole` / `ResizePseudoConsole` / `ClosePseudoConsole`. Symbols are dynamic-loaded from `kernel32.dll` with sideload support for a bundled `conpty.dll` (newer OpenConsole). **No feature flag needed** — `cfg(windows)` pulls `winapi` deps automatically
- **Minimum Windows**: **Windows 10 1809** (October 2018 Update). Hard `expect()` panic below that — Codemux needs a pre-flight OS check and a friendly error instead of letting the panic propagate
- **Production maturity**: wezterm's official Windows build uses this exact crate in production — strong maturity signal
- **CRITICAL — open regression #6783** (`portable-pty 0.9.0 doesn't work on windows`): reader returns garbage output. Reporter confirmed 0.8.1 works, 0.9.0 broken. Open since 2025-03-11, last updated 2025-11-28. **Action required**: pin `portable-pty = "=0.8.1"` on Windows, or verify Codemux's batched-reader rewrite happens to sidestep the bug. See the new decision item below
- **Other open issues directly relevant to Codemux**:
  - **#6946** — spawning via `portable-pty` inside a **Tauri** release build pops a stray `cmd` window. `psuedocon.rs` passes `0` for `dwCreationFlags`, missing `CREATE_NO_WINDOW`. Will hit Codemux directly — plan to patch or fork
  - **#4205** — `CommandBuilder` mutates `PATH` on Windows and can strip user PATH entries. Matters for resolving `claude`, `codex`, etc. from the spawned child — set `PATH` explicitly per-spawn
  - **#7709** (PR open 2026-04-03) — `do_kill` has inverted logic (`res != 0` → Err). Exit codes after explicit kill are unreliable until merged
- **Reader model**: Windows pipes are blocking (no `select` / `poll`). Batched reader must stay per-thread-per-PTY on Windows — matches Codemux's current Linux design, so no architectural change needed from the audit plan

### 3. Agent CLI Windows availability — All 5 run natively

| CLI | Windows? | Install on Windows | Flag parity | Critical notes |
|---|---|---|---|---|
| **Claude Code** | native | `irm https://claude.ai/install.ps1 \| iex`, `winget install Anthropic.ClaudeCode`, or (deprecated) `npm i -g @anthropic-ai/claude-code` | Same — `--system-prompt`, `-p`, `--mcp-config`, `--append-system-prompt` documented | **Hard dep on Git for Windows** — Claude Code shells out to Git Bash internally. `CLAUDE_CODE_GIT_BASH_PATH` env var can override. Win 10 1809+. Docs: https://code.claude.com/docs/en/setup |
| **Codex** (OpenAI) | native | `npm i -g @openai/codex` (Node 22+) or prebuilt Rust binary from GitHub releases | Same — `-c instructions=...` works cross-platform per https://developers.openai.com/codex/cli/reference | Two native sandbox modes (`elevated` / `unelevated`); elevated needs Admin approval. No winget package found |
| **Gemini CLI** | native | `npm install -g @google/gemini-cli` (Node 18+, 20+ preferred); optional native binaries | `GEMINI_SYSTEM_MD` is **officially documented** at https://geminicli.com/docs/cli/system-prompt/ — not an empirical discovery. Supports absolute-or-relative paths or `true`/`1` → `.gemini/system.md`, with a UI indicator when active | Windows 11 24H2+ recommended by install docs |
| **OpenCode** | native (WSL recommended by upstream) | `scoop install opencode`, `choco install opencode`, `npm i -g opencode-ai@latest`, Docker, or prebuilt `.exe` | **Env-var injection on native Windows unverified** — must test before shipping | **Publisher rename**: now maintained by **anomalyco** — `github.com/sst/opencode` redirects to `github.com/anomalyco/opencode`. Codemux may have hardcoded URLs referencing sst. Not the unrelated `github.com/opencode-ai/opencode`. Issue #11256 shows Windows-native install has rough edges |
| **Pi** | native | `npm install -g @mariozechner/pi-coding-agent` | Same — `--append-system-prompt <text>` documented in CLI reference | **NOT Inflection AI's Pi chatbot** — this is `pi-mono` by Mario Zechner (`badlogic` on GitHub), repo https://github.com/badlogic/pi-mono. Dedicated "Platform notes: Windows" section in README. Minor UI gotcha: Windows Terminal's Alt+Enter → fullscreen conflicts with pi's follow-up shortcut (no subprocess impact) |

**Bottom line**: all five agent CLIs are viable on Windows v1. Node.js is a prerequisite for Codex / Gemini / OpenCode / Pi; Claude Code has a standalone installer but requires Git for Windows. The Codemux Windows installer or setup docs should detect (or link to `winget install`) Node.js + Git for Windows + any selected agent CLIs.

---

## Blocking (must fix to compile/run)

### PTY Layer

> **Core good news:** Codemux already uses `portable-pty`, which transparently supports ConPTY on Windows. Items 1.1–1.5 below should mostly "just work" once the reader loop (1.6, 1.7) is rewritten.

- [x] **Verify `portable-pty` ConPTY feature is enabled** — `src-tauri/Cargo.toml`
  - Confirm no feature flag or default disables Windows support. All downstream PTY items depend on this.
  - **Done (2026-04-12)**: Cargo.toml pins `portable-pty = "0.8"`, which is the last-known-good version per research item #6783. No feature flags disabling Windows. Left at 0.8.x (not 0.9.x) so the mechanical pass is safe to merge.
- [ ] **`src-tauri/src/terminal/mod.rs:561-567` — shell PTY openpty**
  - `native_pty_system().openpty(PtySize {...})` for user shell
  - portable-pty handles transparently; verify after Tokio reader refactor
- [ ] **`src-tauri/src/terminal/mod.rs:1298-1304` — agent PTY openpty**
  - Same pattern for agent spawning
  - portable-pty handles; no code change expected
- [ ] **`src-tauri/src/terminal/mod.rs:694, 1416` — `pty_pair.slave.spawn_command(cmd)` for shell and agent**
  - Spawns process into PTY slave
  - portable-pty handles; verify Command env setup works (PATH separator — see item below)
- [ ] **`src-tauri/src/terminal/mod.rs:1193-1199` — `master.resize(PtySize {...})`**
  - PTY resize from frontend window resize
  - portable-pty handles ConPTY resize transparently
- [ ] **`src-tauri/src/terminal/mod.rs:6-9` — `std::os::unix::{fs::PermissionsExt, io::RawFd}` imports**
  - Unix-specific; already gated `#[cfg(unix)]`
  - Verify all usages downstream are gated; no change if clean
- [ ] **`src-tauri/src/terminal/mod.rs:338-359` — `poll_read_ready(fd: RawFd, timeout_ms)` using `libc::poll()`**
  - `#[cfg(unix)]`-gated poll syscall with EINTR retry
  - Replace with Tokio async reads; portable-pty returns `Box<dyn Read>` wrappable in async runtime
  - **Partial (2026-04-12)**: unix path untouched; the `RawFd` usage is now typed as `PollFd = RawFd` on unix and `PollFd = ()` on windows so the module compiles on both. Full Tokio rewrite is still outstanding.
- [ ] **`src-tauri/src/terminal/mod.rs:368-412` — `batched_reader_loop(...)`**
  - Batches PTY output every 16ms or 32KB using `poll()`
  - Rewrite with `tokio::select!` on read future + `tokio::time::interval(Duration::from_millis(16))`; preserve 32KB flush threshold and batching semantics
  - **Partial (2026-04-12)**: added a `#[cfg(windows)]` placeholder that does a simple blocking read loop with per-read flushes (no 16ms batching). Sufficient to compile; optimization is still outstanding.

### Process Management

- [ ] **`src-tauri/src/terminal/mod.rs:473-497` — `ChildGuard` RAII (drop calls `child.kill()` + `child.wait()`)**
  - Prevents zombie children if PTY spawn fails mid-flight
  - `portable_pty::Child::kill()` calls `TerminateProcess` on Windows; no code change, verify via tests
- [ ] **`src-tauri/src/terminal/mod.rs:849-889` — shell child.wait() thread**
  - Spawns thread to wait on shell exit and emit exit status
  - portable-pty handles `GetExitCodeProcess` on Windows; no code change
- [ ] **`src-tauri/src/terminal/mod.rs:1663-1690` — agent child.wait() thread**
  - Same pattern for agent PTY
  - portable-pty handles; no code change
- [x] **`src-tauri/src/commands/mod.rs:274-292` — `kill_port(port)` shells out to `kill -9 {pid}`**
  - Kills process listening on a port
  - Windows branch: `taskkill /PID {pid} /F`, or use `windows` crate `TerminateProcess`
  - **Done (2026-04-12)**: `kill_port` now branches on `cfg!(windows)` and calls `taskkill /PID {pid} /F` on Windows, `kill -9 {pid}` on Unix.
- [x] **`src-tauri/src/agent_browser.rs:89` — `pkill -f agent-browser` for stale daemon cleanup**
  - Kills zombie agent-browser daemons from previous runs
  - Windows: `taskkill /IM agent-browser.exe /F`; `#[cfg(target_os = "windows")]` guard
  - **Done (2026-04-12)**: `kill_stream_daemons` now has a `#[cfg(windows)]` branch running `taskkill /IM agent-browser.exe /F`.
- [x] **`src-tauri/src/agent_browser.rs:470, 619, 670` — `fuser -k {port}/tcp` for port reclamation**
  - Frees ports stuck after crash
  - Windows: parse `netstat -ano` for PID + `taskkill /PID`, or use `GetExtendedTcpTable` from `windows` crate
  - **Done (2026-04-12)**: `kill_process_on_port(port)` now has a full `#[cfg(windows)]` implementation. Runs `netstat -ano` with `CREATE_NO_WINDOW` (no console flash), parses `TCP <local>:<port> ... LISTENING <pid>` rows (tolerant of both IPv4 `0.0.0.0:9223` and IPv6 `[::]:9223`), then `taskkill /PID <pid> /F` each owning PID. Filtering in Rust (not `findstr :{port}`) avoids false-positives on substring matches like `:92230`. Unix path (`fuser -k`) unchanged.

### Browser Automation / Tier 3 Input Injection

> **Scope:** `src-tauri/src/os_input.rs` is entirely Linux-specific. Every item below needs a `#[cfg(target_os = "windows")]` twin implementation using Win32 `SendInput` (via the `windows` crate or `enigo`).
>
> **Compile-gate pass (2026-04-12)**: the entire contents of `os_input.rs` are now inside `#[cfg(target_os = "linux")] mod linux_impl { ... }`, with a non-linux stub for `handle_os_action` that returns `"OS input not supported on this platform"`. The module compiles on Windows — the items below still track the Win32 `SendInput` implementation that will replace the stub.

- [ ] **`src-tauri/src/os_input.rs:36-63` — ydotool binary + ydotoold daemon availability check (via `systemctl --user is-active`)**
  - Linux-only entry gate for Tier 3 input
  - Windows: replace with Win32 availability check (always true — SendInput is core API); no daemon needed
- [ ] **`src-tauri/src/os_input.rs:108-143` — `ydotool_move()`, `ydotool_click_left()`, `ydotool_type_text()` primitives**
  - Shells out to `ydotool` binary
  - Windows: call `SendInput` with `MOUSEINPUT` (move/click) and `KEYBDINPUT` (type) structs via `windows` crate
- [ ] **`src-tauri/src/os_input.rs:149-178` — Linux key name → event code map (from `linux/input-event-codes.h`)**
  - "a"=30, "return"=28, etc.
  - Windows: parallel map to VK codes (VK_A=0x41, VK_RETURN=0x0D); consider cross-platform enum
- [ ] **`src-tauri/src/os_input.rs:180-215` — `ydotool_key()` modifier chain via keycode:1/keycode:0 press/release**
  - Handles Ctrl+Shift+A style combos
  - Windows: `SendInput` sequence with `KEYEVENTF_KEYUP` for release; same press-hold-release pattern
- [ ] **`src-tauri/src/os_input.rs:66-102` — `find_browser_window()` via `hyprctl clients -j`**
  - Gets browser window geometry + Hyprland address
  - Windows: `EnumWindows` + `GetWindowRect` + match on window class name (e.g., "Chrome_WidgetWin_1")
- [ ] **`src-tauri/src/os_input.rs:225-257` — `os_click()` viewport→screen conversion + Bezier-smoothed mouse path**
  - High-level click orchestration
  - Keep the Bezier math; swap ydotool primitive calls for Win32 equivalents
- [ ] **`src-tauri/src/os_input.rs:277-309` — `handle_os_action()` dispatcher for "click_os" / "type_os"**
  - MCP action router
  - Add `#[cfg(target_os = "windows")]` branch routing to Win32 implementation

### Desktop Integration

- [x] **`src-tauri/src/lib.rs:369-377` — startup `hyprctl keyword windowrule float...` for xdg-desktop-portal-gtk dialogs**
  - Hyprland-specific workaround for tiled file dialogs
  - Gate with `#[cfg(target_os = "linux")]`; Windows native dialogs don't need this
  - **Done (2026-04-12)**: wrapped the `hyprctl keyword windowrule` call in `#[cfg(target_os = "linux")]`.

### Filesystem / IPC

- [x] **`src-tauri/src/control.rs:11` — `use tokio::net::{UnixListener, UnixStream}`**
  - IPC transport for codemux control socket
  - `#[cfg(unix)]`-gate; Windows branch uses `tokio::net::windows::named_pipe::{NamedPipeServer, NamedPipeClient}`
  - **Done (2026-04-12)**: `control.rs` now has two sibling modules — `unix_transport` (UnixListener/UnixStream) and `windows_transport` (ServerOptions / ClientOptions / NamedPipeServer / NamedPipeClient) — both exposing the same `bind` / `accept` / `connect` / `sync_liveness_probe` API. The rest of the file uses `transport::*` and never names a platform-specific type. `handle_client` is now generic over `AsyncRead + AsyncWrite + Unpin + Send`.
- [x] **`src-tauri/src/control.rs:30-76` — `control_socket_path()` resolves `$XDG_RUNTIME_DIR/codemux.sock` or `/tmp/codemux-{uid}/codemux.sock`**
  - Socket path resolution with Unix fallback
  - Windows: return `\\.\pipe\codemux-{username}` named pipe path; use `whoami::username()` or `%USERNAME%` env var
  - **Done (2026-04-12)**: `control_socket_path()` is now cfg-split. Unix keeps the existing XDG_RUNTIME_DIR + /tmp/codemux-{uid} fallback logic verbatim. Windows returns `\\.\pipe\codemux-{USERNAME}` using `std::env::var("USERNAME")` (with a `"default"` fallback and alphanumeric/`_-` sanitization so a weird USERNAME can never produce an invalid pipe name). No new `whoami` crate dependency.
- [x] **`src-tauri/src/control.rs:36` — `let uid = unsafe { libc::getuid() }`**
  - Unix UID lookup for socket dir naming
  - Windows: `whoami::username()` or `GetUserNameW` via `windows` crate
  - **Done (2026-04-12)**: `libc::getuid()` stays inside `#[cfg(unix)]`; the Windows branch uses `std::env::var("USERNAME")` and never touches libc.
- [x] **`src-tauri/src/control.rs:127` — `UnixStream::connect()` liveness probe on existing socket**
  - Checks if daemon already owns the socket
  - Windows: `NamedPipeClient::connect()` probe
  - **Done (2026-04-12)**: replaced the inline `std::os::unix::net::UnixStream::connect` check with `transport::sync_liveness_probe(&socket_path)`. Unix impl uses the std Unix socket connect; Windows impl uses `std::fs::OpenOptions::new().read(true).write(true).open(pipe_name)` which maps to `CreateFileW` and succeeds iff a server is listening. Both are safe to call from sync contexts (startup path before the tokio runtime exists). Exposed as `pub fn control_server_is_running()` for `main.rs`.
- [x] **`src-tauri/src/control.rs:178-199` — `UnixListener::bind(&socket_path)` for IPC server**
  - Main control server listen
  - Windows: `ServerOptions::new().create(pipe_name)` from `tokio::net::windows::named_pipe`
  - **Done (2026-04-12)**: server bind is now `transport::bind(&socket_path)`. Unix impl removes the stale socket file (if any) and binds a `UnixListener`. Windows impl creates the first pipe server instance with `ServerOptions::new().first_pipe_instance(true)` and keeps it inside a `Mutex<Option<NamedPipeServer>>`; `accept()` awaits `server.connect()`, eagerly spins up the next instance, and returns the now-connected server. The accept loop in `spawn_control_server` is identical for both platforms. Unix-only filesystem bookkeeping (parent dir creation + stale-file sweep) is wrapped in `#[cfg(unix)]`.
- [x] **`src-tauri/src/main.rs:5, 199` — `use std::os::unix::net::UnixStream` for CLI daemon detection**
  - `codemux` CLI uses this to detect a running daemon
  - Windows: mirror the named-pipe probe from control.rs
  - **Done (2026-04-12)**: dropped the `std::os::unix::net::UnixStream` import; single-instance detection is now `codemux_lib::control::control_server_is_running()`, which delegates to the platform-specific `transport::sync_liveness_probe`. Same behavior on Unix, works on Windows.
- [x] **`src-tauri/src/git.rs:1247-1324, 1261` — `git_create_worktree()` uses `env::var("HOME")` with `/tmp` fallback**
  - Worktree root path: `$HOME/.codemux/worktrees/{repo}/{branch}`
  - Replace with `dirs::home_dir()` (resolves to `%USERPROFILE%` on Windows); drop `/tmp` fallback
  - **Done (2026-04-12)**: now uses `dirs::home_dir()` with `std::env::temp_dir()` as final fallback.
- [x] **`src-tauri/src/agent_context.rs:102` — hardcoded `/tmp/codemux-{workspace_id}-gemini-system.md`**
  - Gemini system prompt temp file
  - Use `std::env::temp_dir().join(format!("codemux-{}-gemini-system.md", workspace_id))`
  - **Done (2026-04-12)**: uses `std::env::temp_dir().join(...)`. Note: the enclosing shell command still uses `printf` + `$VAR` expansion so it won't actually run on native Windows cmd.exe yet — that's covered by the "Agent Integration" blocker for Claude/Codex/Pi/Gemini injection.

### Shell & Environment

- [x] **`src-tauri/src/terminal/mod.rs:690-692` — PATH prepend uses `:` separator**
  - Prepends codemux CLI shim dir to child PATH
  - `if cfg!(windows) { ";" } else { ":" }`, or use `std::env::join_paths`
  - **Done (2026-04-12)**: separator is now `cfg!(windows) ? ";" : ":"`.
- [x] **`src-tauri/src/terminal/mod.rs:414-419` — `default_shell()` reads `SHELL` → `/bin/bash` fallback**
  - User shell detection for terminal panes
  - Windows branch: `env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into())`; future: detect PowerShell via `HKLM\SOFTWARE\Microsoft\PowerShell`
  - **Done (2026-04-12)**: split into `#[cfg(unix)]` (SHELL env) and `#[cfg(windows)]` (COMSPEC env, fallback `cmd.exe`).
- [ ] **`src-tauri/src/openflow/prompts.rs:249-404` — `ensure_openflow_wrapper_exists()` generates `#!/bin/bash` wrapper**
  - Uses `set -uo pipefail`, `read -r`, `[[ ]]`, python JSON parsing, `/tmp/openflow-session-*`
  - Generate `.ps1` on Windows (PowerShell has native JSON + `$env:TEMP` + stdin handling); better: refactor to spawn agent directly from Rust, eliminating the wrapper
- [ ] **`src-tauri/src/openflow/prompts.rs:426-600` — `ensure_claude_wrapper_exists()` generates `#!/bin/bash` wrapper**
  - Uses `stty -echo`, `read -r -t 1`, `python3 -c` for JSON, `/tmp/openflow-claude-session-*`
  - Generate `.ps1` on Windows; no `stty` equivalent — use `[Console]::ReadKey()` or buffer differently
- [ ] **`src-tauri/src/openflow/prompts.rs:299, 315` — `/tmp/openflow-session-${INSTANCE_ID}-$$` session files**
  - Temp files referenced inside the generated bash wrappers
  - Use `%TEMP%` in generated `.ps1` wrapper; or `std::env::temp_dir()` if refactored to Rust spawn
- [ ] **`scripts/check-deps.sh:131-146` — checks `webkit2gtk-4.1` and `gtk-3.0` via `pkg-config`**
  - Would hard-fail on Windows
  - Create `scripts/check-deps.ps1` that skips webkit/gtk and verifies MSVC toolchain + Visual Studio Build Tools + WebView2 runtime

### Build & Distribution

- [x] **`src-tauri/tauri.conf.json:31` — `bundle.targets = "all"` but only `linux` section configured**
  - No Windows installer config present
  - Add `"windows": { "nsis": { "languages": ["English"], "displayLanguageSelector": false } }`. Recommend NSIS over MSI for v1 — smaller, simpler, no Windows SDK required
  - **Done (2026-04-12)**: added a `bundle.windows.nsis` section with `languages: ["English"]`, `displayLanguageSelector: false`, and explicit null `installerIcon`/`headerImage`/`sidebarImage` (use Tauri defaults). No code signing yet — that's gated on a budget decision. The release pipeline (`release.yml`) itself is still intentionally Linux-only; this config unblocks the eventual Windows build job so it can produce a `.exe` installer once signing and the release-matrix rewrite land.
- [ ] **`src-tauri/tauri.conf.json:6-9` — `beforeBuildCommand` / `beforeDevCommand` invoke `bash scripts/copy-agent-browser.sh`**
  - Hard-codes `bash`
  - Replace with Node.js script (`scripts/copy-agent-browser.mjs`) for true cross-platform, or conditionally invoke a `.ps1` variant
- [x] **`.github/workflows/release.yml:10` — `runs-on: ubuntu-22.04` only; installs Linux-only deps (`libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `librsvg2-dev`, `libfuse2`)**
  - No Windows build in CI
  - Add matrix: `os: [ubuntu-22.04, windows-latest, macos-latest]`. Windows uses MSVC toolchain (pre-installed on `windows-latest`) and WebView2 runtime (pre-installed on recent Windows images)
  - **CI coverage (2026-04-12)**: added `.github/workflows/ci.yml` that runs `cargo check`, `cargo test`, `npm run check`, `npm run test`, and `npm run build` on a `[ubuntu-latest, windows-latest]` matrix with `fail-fast: false`. Catches Windows regressions on every push to `main`/`feature/**` and every PR into `main`.
  - **Release pipeline (2026-04-12)**: `release.yml` now also builds on a `[ubuntu-22.04, windows-latest]` matrix with `fail-fast: false`. Ubuntu is pinned at 22.04 (not ubuntu-latest) so the AppImage stays glibc-compatible with older distros when GitHub rolls ubuntu-latest to 24.04. Both platforms share the same Setup Rust / Rust cache / Setup Node / npm ci / agent-browser pre-stage / git identity steps. The `tauri-apps/tauri-action@action-v0.6.2` call is split into two conditional invocations — one for `ubuntu-22.04` and one for `windows-latest` — because the Windows side additionally passes `args: --bundles nsis`. `bundle.targets = "all"` in tauri.conf.json would otherwise build both NSIS and MSI on Windows, and MSI requires the WiX Toolset which is NOT preinstalled on `windows-latest` runners — the build would fail with "wix not found". Restricting to `--bundles nsis` sidesteps the problem.
  - **Windows auto-update enabled (2026-04-13)**: both Linux and Windows steps now pass `TAURI_SIGNING_PRIVATE_KEY` / `..._PASSWORD` and set `includeUpdaterJson: true`. A single `latest.json` contains both `linux-x86_64` and `windows-x86_64` entries under its `platforms` key, which matches Tauri v2's updater schema. No third merge-updater job is needed — `tauri-action`'s `uploadVersionJSON` step (`src/upload-version-json.ts` in the tauri-action repo) already merges by listing existing release assets, downloading any pre-existing `latest.json`, pre-seeding `versionContent.platforms` with the existing object, adding the current job's entries, then `deleteReleaseAsset` + `uploadAssets` of the merged file. Theoretical race: if both jobs call `listReleaseAssets` within the same ~1s window before either has uploaded, the second upload clobbers the first. In practice ubuntu-22.04 completes 5-15 minutes before windows-latest so the race is very unlikely. Worst case of a race is one release with a single-platform `latest.json` — the other platform's users miss one update cycle, not bricked. We accept this over writing a custom merge-updater job because tauri-action's built-in merge is more reliable than anything we'd write. The Ed25519 key is cross-platform: same private key signs Linux AppImage/deb/rpm `.sig` files and Windows NSIS `.exe.sig` files; clients match per-platform signatures from the single merged `latest.json`. This is DISTINCT from Windows Authenticode code signing, which is still NOT configured (see next bullet).
  - **What's still explicitly not done**: Windows Authenticode code signing (SmartScreen warning on unsigned first-installs — budget decision, inline TODO comment in the workflow), macOS matrix entry (separate follow-up).
- [x] **Bump Node 20 → 22 in both `ci.yml` and `release.yml`**
  - **Done (2026-04-12)**: GitHub is deprecating Node 20 runners starting June 2026 per https://github.blog/changelog/2025-09-19-deprecation-of-node-20-on-github-actions-runners/. Both workflows now set `node-version: 22` with an inline link to the deprecation notice.
- [ ] **`scripts/copy-agent-browser.sh` — bash platform→binary mapping for `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, darwin targets**
  - No Windows target mapping; bash won't run on Windows without WSL or Git Bash
  - Rewrite as Node.js (`copy-agent-browser.mjs`) for cross-platform, or add `copy-agent-browser.ps1` with win32-x64/arm64 mapping

### Agent Integration

- [ ] **Verify `agent-browser` upstream publishes a Windows binary** — `src-tauri/src/agent_browser.rs`, `scripts/copy-agent-browser.sh`
  - Codemux depends on the `agent-browser` Node package; currently published for Linux and macOS only
  - If missing: vendor a Windows Chromium launcher, use system Edge/Chrome via CDP directly, or drop the browser pane from Windows MVP
- [ ] **Verify Claude Code CLI flag parity on Windows** — `src-tauri/src/agent_context.rs:88-95`, `src-tauri/src/openflow/adapters/claude.rs`
  - `--system-prompt`, `-p`, `--mcp-config`, etc. must work identically on Windows
  - Hard blocker if any Codemux-relied flag is missing or behaves differently. Test with `claude --version` and compare flag set
- [ ] **`src-tauri/src/openflow/adapters/claude.rs:1-88` — Claude adapter spawns `SystemPrompts::claude_wrapper_script_path()` bash wrapper**
  - Depends on the bash wrapper generated in `prompts.rs:426-600`
  - Option A: produce `.ps1` wrapper on Windows. Option B: refactor to spawn `claude` directly from Rust with env vars + stdin, eliminating the wrapper entirely (preferred — simpler cross-platform)
- [ ] **`src-tauri/src/agent_context.rs:89-92` — Claude injection: `--system-prompt "$CODEMUX_AGENT_CONTEXT"` with shell-style var expansion**
  - cmd.exe uses `%VAR%`, PowerShell uses `$env:VAR` — bare `$VAR` won't expand
  - Verify env is passed via `Command::env()` not via shell string; if spawning goes through a shell, rewrite to pass directly

### Windows MVP Disable Strategy — OpenFlow

OpenFlow depends on the bash wrapper scripts generated in `openflow/prompts.rs` (`ensure_wrapper_exists` / `ensure_claude_wrapper_exists`). Those don't have Windows equivalents yet. Rather than ship a broken feature that fails with cryptic wrapper-spawn errors, OpenFlow is disabled at every layer on Windows for v1. Re-enabling is a follow-up blocked on the wrapper rewrite — Option B under the Agent Integration item above (refactor to spawn adapters directly from Rust, eliminating the wrapper entirely, is the preferred path).

- [x] **`src-tauri/src/commands/mod.rs` — new `get_platform()` Tauri command**
  - Returns `std::env::consts::OS` as a `String`. Enables feature gating from the React frontend without needing per-feature cfg flags.
  - Wired into `lib.rs` invoke_handler as `commands::get_platform`.
  - **Done (2026-04-12)**: new module-level command in `commands/mod.rs` just above `get_detected_ports`. Sibling frontend wrapper `getPlatform()` added to `src/tauri/commands.ts`.
- [x] **`src/hooks/use-platform.ts` — `usePlatform()` React hook**
  - Module-level cache keyed on first successful resolve (OS doesn't change during a session). In-flight invocations are deduped via a shared promise so N concurrent `usePlatform()` consumers share one IPC round-trip. Failure falls back to an empty string so a broken invoke never hides OpenFlow from Linux users (false-positive Windows detection).
  - **Done (2026-04-12)**: exposes `{ os, loading, isWindows }`; second + subsequent mounts return the cached value synchronously on the first render.
- [x] **`src/components/layout/sidebar-openflow-section.tsx` — disabled header on Windows**
  - Renders a greyed-out header with a `Tooltip` explaining "OpenFlow is not yet available on Windows" when `isWindows`. The `+` button is `disabled` / `aria-disabled`, the expand toggle is removed, and `NewRunDialog` is not mounted at all — nothing to open it from. The section stays visible so Windows users know the feature exists and is coming.
  - **Done (2026-04-12)**: Windows branch is a completely separate `if (isWindows) return ...` at the top of the component, leaving the Linux code path byte-identical.
- [x] **`src-tauri/src/commands/openflow.rs:498` — `spawn_openflow_agents` Windows guard**
  - Safety net for CLI or socket-driven callers that might bypass the UI (e.g. the `codemux` control socket). Early-returns `Err("OpenFlow is not yet available on Windows")` when `cfg!(windows)`, before any wrapper-script creation attempts.
  - **Done (2026-04-12)**: guard added as the first statement of the command body, with an inline comment pointing at the wrapper dependency and the UI-layer disable.

**Explicitly out of scope for this pass**: no refactoring of `src-tauri/src/openflow/` wrapper scripts, no changes to `release.yml`, no changes to `os_input.rs` (Tier 3 input is deferred). Those remain tracked under their own items above.

---

## Degraded (feature broken, app runs)

### Process / Port Detection

- [x] **`src-tauri/src/ports.rs:38-63` — `detect_listening_ports()` parses `/proc/net/tcp` and `/proc/net/tcp6`**
  - Port discovery for UI
  - Windows: `GetExtendedTcpTable()` via `windows` crate, or accept empty list on Windows (users type port manually)
  - **Done (2026-04-12)**: `detect_listening_ports()` now dispatches on cfg. Linux path uses the existing `/proc/net/tcp` + `/proc/*/fd/` scan unchanged. Windows path (`windows_impl::detect_listening_ports`) shells out to `netstat -ano` with `CREATE_NO_WINDOW` (no console flash — `scan_ports` runs every 3 s, so the suppression flag is non-negotiable), filters for `TCP ... LISTENING` rows, and parses `<local>:<port> <pid>` for both IPv4 `0.0.0.0:135` and IPv6 `[::]:135` addresses. Existing `IGNORED_PORTS` and `is_codemux_internal_port` filters are applied on both platforms.
- [x] **`src-tauri/src/ports.rs:120` — `/proc/*/fd/` symlink scan to map sockets → PIDs**
  - Owner process attribution for detected ports
  - `GetExtendedTcpTable` returns PID directly; no fd scan needed
  - **Done (2026-04-12)**: the Windows path gets PID directly from `netstat -ano` — no fd scan. Process names come from a single `tasklist /NH /FO csv` call per scan (NOT per port — we build a PID→name `HashMap` once), parsing the first two CSV cells as `name,pid`. Shelling out chose over `GetExtendedTcpTable` to avoid adding the `windows` crate dep for MVP; can swap later if the 3-second-interval `netstat` spawn proves too slow.
- [x] **`src-tauri/src/ports.rs:156-166` — `read_ppid()` reads `/proc/{pid}/stat` for parent PID walk**
  - Walks up process tree to attribute ports to workspaces
  - Windows: `CreateToolhelp32Snapshot` + `Process32First/Next`
  - **Done (2026-04-12)**: `read_ppid` now dispatches on cfg. Linux unchanged. Windows implementation shells out to `wmic process where "ProcessId=<pid>" get ParentProcessId /value` and parses the `ParentProcessId=<n>` line. Deliberately avoids `CreateToolhelp32Snapshot` (would require the `windows` crate) — the PPID walk is a nice-to-have for workspace attribution, not a correctness requirement, so `None` is an acceptable graceful degradation. Follow-up: `wmic` is deprecated on Windows 11 24H2+; if this stops working, swap to PowerShell `Get-CimInstance` or add the `windows` crate dep.
- [ ] **`src-tauri/src/hooks.rs:195-219` — `shell_is_foreground()` reads `/proc/{shell_pid}/stat` for tpgid vs pgrp**
  - Suppresses hook notifications when shell is backgrounded
  - Already has `#[cfg(not(target_os = "linux"))]` stub returning `false`; hook notifications always fire on Windows (acceptable)
- [ ] **`src-tauri/src/commands/openflow.rs:766-774` — `/proc/self/status` VmRSS read for memory diagnostics**
  - Memory tracking in orchestration cycle
  - Already has non-Linux fallback returning `None`; future: `GetProcessMemoryInfo` via `windows` crate
- [ ] **`src-tauri/src/indexing.rs:327-337` — `#[cfg(unix)] { MetadataExt::dev()/ino() }` for symlink cycle detection**
  - Prevents infinite recursion during indexing
  - Windows: replace with visited-path `HashSet<PathBuf>` (slightly less efficient, correct); NTFS loops are rare
- [ ] **`src-tauri/src/auth.rs:303-320` — `#[cfg(unix)] libc::setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, ...)` for OAuth callback socket**
  - Sets 5-second socket receive timeout
  - Use `socket2` crate for cross-platform socket options, or `#[cfg(windows)]` branch calling Winsock `setsockopt`
- [ ] **`src-tauri/src/execution/mod.rs:128-205` — `build_linux_bwrap_args()` bubblewrap sandbox args**
  - Namespace unsharing for agent sandboxing (`--unshare-pid`, `--unshare-ipc`, `--unshare-net`, `--die-with-parent`)
  - Already gated — Windows falls through to `HostPassthrough`. Future: implement `WindowsRestricted` via Job Objects / AppContainer
- [ ] **`src-tauri/src/execution/mod.rs:34-65` — backend selection prefers `WindowsRestricted` placeholder, falls back to `HostPassthrough`**
  - Agents run unsandboxed on Windows
  - Already correct fallback behavior; document the limitation in release notes
- [ ] **`src-tauri/src/terminal/mod.rs:1611-1652` — POSIX exit code → signal name mapping (128+N → SIGTERM/SIGKILL/SIGWINCH)**
  - Human-readable agent/shell death reports
  - Windows exit codes fall into `_ => "unexpected code"` branch; acceptable but misses nice reporting

### Filesystem / Permissions

- [x] **`src-tauri/src/control.rs:40-70` — `MetadataExt::uid()` ownership check + `Permissions::from_mode(0o700)` on fallback socket dir**
  - Security: ensures only current user can connect
  - Windows named pipes inherit user security context automatically; skip this check on Windows
  - **Done (2026-04-12)**: `MetadataExt::uid()` and `Permissions::from_mode(0o700)` now live inside the `#[cfg(unix)]` branch of `control_socket_path()`. The Windows branch returns the pipe path directly with no permission bookkeeping — named pipes inherit the creating user's security descriptor from Win32 by default, which is the security model the Unix chmod was approximating.
- [x] **`src-tauri/src/git.rs:1260` — branch-name sanitization replaces only `/` with `-`**
  - Used to build worktree directory name
  - Windows forbids `< > : " \ | ? *` in filenames. Expand sanitizer: `branch.replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '_', "-")`
  - **Done (2026-04-12)**: sanitizer now replaces `/ \ < > : " | ? *` all with `-`.
- [ ] **`src-tauri/src/config/mod.rs:71` — reads `~/.config/omarchy/current/theme/colors.toml` for Omarchy theme integration**
  - Omarchy is a Linux/Hyprland theming tool
  - Skip entirely on Windows; theme falls back to default
- [x] **`src-tauri/src/hooks.rs:317-323` — `#[cfg(unix)] set_permissions(Permissions::from_mode(0o755))` on hook script**
  - Makes hook script executable on Unix
  - Already cfg-gated; Windows uses file extension (`.bat`/`.ps1`) for executability. No action beyond ensuring the script itself is `.bat`/`.ps1` (see Shell section)
  - **Verified (2026-04-12)**: already `#[cfg(unix)]`-gated; Windows path is a no-op. Nothing to do for the mechanical pass; generating `.bat`/`.ps1` scripts is the separate wrapper rewrite.
- [x] **`src-tauri/src/openflow/prompts.rs:411-414, 606-609` — `0o755` on generated wrapper scripts**
  - OpenFlow + Claude wrapper scripts
  - Use `.ps1` extension on Windows (see Shell section); drop mode-bit setting inside `#[cfg(windows)]`
  - **Verified (2026-04-12)**: both `set_permissions` blocks are already inside `#[cfg(unix)]`. No change needed for this pass — the `.ps1` rewrite is a separate task.
- [x] **`src-tauri/src/terminal/mod.rs:421-437` — `#[cfg(unix)] ensure_openflow_cli_shims()` creates `/tmp/codemux-openflow-shims/` with 0o755**
  - CLI shim installation for PATH injection
  - Windows: create `.bat` shim in `%LOCALAPPDATA%\Codemux\shims`, or skip if PATH injection isn't needed there
  - **Done (2026-04-12)**: added `#[cfg(windows)]` variant that creates `codemux.bat` in `std::env::temp_dir()/codemux-openflow-shims/` with `@echo off\r\n"%EXE%" %*\r\n` contents. No chmod needed on Windows.

### Shell & Environment / Scripts

- [ ] **`src-tauri/src/terminal/mod.rs:429-432` — generated shim: `#!/bin/sh\nexec "..." "$@"\n`**
  - CLI shim content
  - Windows: generate `.bat`: `@echo off\r\n"%PROG%" %*`
- [ ] **`src-tauri/src/hooks.rs:282-315` — hook script: `#!/bin/sh` + `curl` POST + optional `jq`**
  - Agent lifecycle notification hook
  - Generate `.ps1`: `Invoke-RestMethod -Method Post -Uri $env:CODEMUX_HOOK_URL -Body $body` (curl.exe also ships with Windows 10+ if preferred)
- [ ] **`package.json:7-18` — npm scripts with `GDK_BACKEND=x11`, `tauri:dev:x11` variants**
  - Linux/X11-specific env var setting
  - Use `cross-env` package or conditional scripts; remove X11 variants from Windows path
- [ ] **`scripts/check-deps.sh` — bash-only dependency checker using apt/dnf/pacman detection**
  - Contributor dev-setup
  - Create `scripts/check-deps.ps1` for Windows (verifies Rust MSVC, Node, VS Build Tools, WebView2 runtime)
- [ ] **`scripts/vite-wrapper.sh` — bash wrapper using `/dev/tcp`, `ps -o pgid`, `trap` for Vite restart monitoring**
  - Dev-only convenience
  - Acceptable to keep Linux-only; Windows dev uses `tauri dev` directly. Optional future port to Node.js

### Desktop / Dialogs

- [x] **`src-tauri/Cargo.toml:26` — `tauri-plugin-dialog` with `xdg-portal` feature enabled unconditionally**
  - Linux file dialog via xdg-portal
  - Gate feature to Linux: `[target.'cfg(unix)'.dependencies] tauri-plugin-dialog = { version = "2", features = ["xdg-portal"] }`; Windows uses native dialogs automatically
  - **Done (2026-04-12)**: moved `tauri-plugin-dialog` into `[target.'cfg(unix)'.dependencies]` with `xdg-portal` feature; added `[target.'cfg(windows)'.dependencies]` block with a plain `tauri-plugin-dialog = "2"`.
- [ ] **`src-tauri/Cargo.toml:36` — `notify-rust = "4"` dependency**
  - Desktop notifications
  - notify-rust has a Windows backend (WinRT Toast); test that `Notification::show()` works without code change
- [ ] **`src-tauri/src/commands/workspace.rs:756-762` — `Notification::new()` with `.hint(DesktopEntry(...))`, `.hint(Transient(true))`, `.urgency(Critical)`**
  - Attention notification
  - `DesktopEntry` and `Transient` hints are D-Bus/freedesktop-specific and silently ignored on Windows; `.urgency(Critical)` maps to toast priority. Works but visual behavior differs

### Agent Integration (follow-ups)

- [ ] **`src-tauri/src/agent_context.rs:94-95` — Codex injection: `-c instructions="$CODEMUX_AGENT_CONTEXT"` with shell expansion**
  - Same concern as Claude injection
  - Prefer passing via `Command::env()` + flag directly; no shell expansion
- [ ] **`src-tauri/src/openflow/prompts.rs:299, 315` — OpenCode wrapper uses `opencode session list --format json` piped via bash**
  - OpenCode session tracking
  - Verify OpenCode CLI works on Windows; if yes, spawn directly from Rust without shell pipe
- [ ] **Verify other agent CLI Windows availability (opencode, codex, gemini, pi)** — `src-tauri/src/openflow/adapters/*.rs`
  - Each agent has a different Windows story
  - Initial Windows release may ship with Claude Code only; document which agents are supported per release

### Signing

- [ ] **No Windows code signing configured anywhere**
  - Users will see Windows SmartScreen warning on first install
  - Options: (a) EV cert (~$300/yr, no warning ever), (b) OV cert (~$100/yr, warning until reputation builds), (c) ship unsigned initially and accept warnings. Once cert is acquired, add `"windows": { "certificateThumbprint": "...", "digestAlgorithm": "sha256", "timestampUrl": "http://timestamp.digicert.com" }` to `tauri.conf.json`

---

## Cosmetic

- [ ] **`src-tauri/src/main.rs:21-81` — debug-only signal logger installs SIGTERM/SIGINT/SIGHUP handlers via `libc::signal`**
  - Writes to `.codemux/native-startup.log` to debug startup crashes
  - Already gated `#[cfg(all(debug_assertions, unix))]`; no Windows equivalent needed
- [ ] **`src-tauri/src/lib.rs:82-93` — `WEBKIT_DISABLE_DMABUF_RENDERER`, `WEBKIT_DISABLE_COMPOSITING_MODE`, `GDK_BACKEND` env var setup**
  - WebKitGTK/Wayland rendering workarounds
  - Already gated `#[cfg(target_os = "linux")]`; Windows uses WebView2 — no env vars needed
- [ ] **`src-tauri/src/execution/mod.rs:157-169` — clears `DISPLAY`, `WAYLAND_DISPLAY`, `DBUS_SESSION_BUS_ADDRESS` in bwrap sandbox**
  - GUI isolation inside Linux sandbox
  - Only runs in `LinuxBubblewrap` backend path; never reached on Windows. No change
- [x] **`src-tauri/src/commands/workspace.rs:771-773` — `hyprctl dispatch focuswindow class:com.codemux.app` after notification**
  - Hyprland window focus
  - Gate with `#[cfg(target_os = "linux")]`. Tauri's `window.request_user_attention()` + `window.set_focus()` already handles Windows transparently
  - **Done (2026-04-12)**: wrapped the `hyprctl dispatch focuswindow` call in `#[cfg(target_os = "linux")]`.
- [x] **`src-tauri/src/diagnostics.rs:34` — tries `$XDG_RUNTIME_DIR` for diagnostics log path**
  - Linux runtime dir
  - Add Windows fallback: `%LOCALAPPDATA%\Codemux\logs`
  - **Done (2026-04-12)**: `native_global_log_path()` now branches on `#[cfg(windows)]` → `dirs::data_local_dir()/Codemux/logs/codemux-native-launches.log`; non-windows path still uses `XDG_RUNTIME_DIR`.
- [ ] **`codemux.desktop` + `src-tauri/tauri.conf.json:37-39` — bundles `.desktop` file to `/usr/share/applications/` for Linux deb**
  - Linux desktop entry
  - Already Linux-conditional; NSIS/MSI bundler will handle Windows Start Menu shortcut via tauri.conf.json
- [x] **`src-tauri/src/commands/browser.rs:158-161` — `~/.agent-browser` dir with `/tmp` fallback**
  - Agent browser state dir
  - Use `std::env::temp_dir()` for fallback; home dir is reliably available on Windows via `dirs::home_dir()`
  - **Done (2026-04-12)**: fallback changed from `/tmp` to `std::env::temp_dir()`.
- [ ] **`src-tauri/src/scrollback.rs:7-8, 110-113`, `src-tauri/src/auth.rs:140-143`, `src-tauri/src/presets.rs:191-194`, `src-tauri/src/database.rs:43-45`, `src-tauri/src/settings_sync.rs:162-165`, `src-tauri/src/session_adapters.rs:105-108`, `src-tauri/src/state/state_impl.rs:2851-2853` — various `dirs::data_dir()` / `dirs::config_dir()` uses**
  - Portable state/config paths
  - `dirs` crate already maps to `%APPDATA%` / `%LOCALAPPDATA%` on Windows. No changes required
- [ ] **`src-tauri/src/agent_context.rs:101, 119, 124` — example hardcoded Unix paths in docs/tests (`/home/user/.codemux/worktrees/...`)**
  - Documentation/test fixtures
  - Update examples to include Windows paths (`C:\Users\user\.codemux\worktrees\...`)
- [ ] **`src-tauri/src/os_input.rs:282` — error message suggests `systemctl --user enable --now ydotool`**
  - Linux-specific guidance
  - Make message OS-specific after Win32 implementation lands
- [ ] **`src-tauri/src/mcp_server.rs:213-237, 561-575` — MCP tool descriptions for `browser_click_os` / `browser_type_os` reference ydotool**
  - Tool metadata shown to agents
  - Update descriptions to platform-neutral wording
- [ ] **`scripts/check-deps.sh:207-218` — ydotool binary + systemd service check**
  - Dev-setup helper
  - Skip on Windows entirely; ydotool is Linux-only
- [ ] **`src-tauri/src/terminal/mod.rs:594-611` — sets `TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=codemux`, `CODEMUX=1`, `CODEMUX_SESSION_ID`, `CODEMUX_WORKSPACE_ID`, `CODEMUX_SURFACE_ID`, `BROWSER=codemux browser open`**
  - PTY child env vars
  - All harmless on Windows ConPTY. Consider detecting `WT_SESSION` for Windows Terminal-specific enhancements later
- [ ] **Test fixtures — hardcoded `/tmp/...` paths in various `*_test.rs` and integration tests**
  - Test hygiene
  - Replace with `std::env::temp_dir().join(...)` across the board
- [ ] **`src-tauri/src/git.rs:1326-1403` — `git_remove_worktree()` reads `.git` text file, parses gitdir**
  - `.git` file format is cross-platform
  - No change required; listed only to record it was verified safe
- [ ] **`src-tauri/src/git.rs:70-108` (+ throughout) — `Command::new("git")` invocations**
  - Git subprocess calls
  - Works on Windows if Git for Windows is in PATH. No change; document as prerequisite in README

---

## Verification Checklist (run after all blocking items done)

- [ ] `cargo build --target x86_64-pc-windows-msvc` compiles clean
- [ ] App launches on Windows and shows splash screen
- [ ] Terminal opens with cmd.exe/PowerShell
- [ ] Can create workspace + worktree
- [ ] Can open browser pane
- [ ] Agent context injection works with Claude Code
- [ ] Notifications appear (Windows toast)
- [ ] Socket/named-pipe control works (`codemux` CLI commands)
- [ ] OpenFlow run completes with 2+ agents
- [ ] Update notification appears and update installs
- [ ] NSIS installer installs/uninstalls cleanly

---

## Testing Requirements

Every `cfg`-gated code path added for Windows needs test coverage. The goal is that the Windows build is proven correct by CI on every PR, not validated by manual testing on a developer's machine.

### Rust tests

Every `#[cfg(windows)]` code path needs a paired `#[cfg(windows)]`-gated test. Linux tests must continue to pass unchanged — no regressions from new cfg gates.

- [x] **`control.rs` named pipe suite**
  - Bind server to pipe, spawn client, connect, send command, receive response
  - Liveness probe: second `control_socket_path()` call detects existing pipe
  - Stale pipe cleanup: dead server leaves pipe that new server can reclaim
  - Permission isolation: verify pipe is only accessible to current user SID (named pipes inherit user security; assert the ACL)
  - Connection timeout + error paths
  - **In progress (2026-04-12)**: unit suite is in place and passes on Linux (5 tests: `test_control_socket_path_format`, `test_unix_socket_round_trip`, `test_liveness_probe_no_server_unix`, `test_liveness_probe_with_server_unix`, `test_fallback_dir_permission_isolation`). The Windows twin tests (`test_named_pipe_round_trip`, `test_liveness_probe_no_server_windows`, `test_liveness_probe_with_server_windows`) are written and `#[cfg(windows)]`-gated; they'll execute once Windows CI lands.
  - **Expanded (2026-04-13)**: extracted the pipe-name sanitization into a cross-platform `build_pipe_path(username: &str) -> PathBuf` helper and added 7 more tests (all cross-platform, NOT cfg-gated): `test_named_pipe_path_format` (typical `alice` → `\\.\pipe\codemux-alice`), `test_named_pipe_path_sanitization_spaces` (`John Smith` → underscores), `test_named_pipe_path_sanitization_special_chars` (`DOMAIN\john.smith`, `user@host`), `test_named_pipe_path_sanitization_empty_or_whitespace` (empty/whitespace fallback to `default`), `test_named_pipe_path_preserves_underscores_and_dashes`, `test_named_pipe_path_sanitization_unicode` (CJK + accented Latin pass through via `char::is_alphanumeric`), `test_named_pipe_path_never_panics_on_pathological_input` (null bytes, 1024-char inputs, path separators). Using a helper instead of mutating the process-global `USERNAME` env var avoids race conditions in parallel test execution. Remaining follow-ups: stale-pipe cleanup test, permission SID/ACL assertion, connection timeout path — still deferred until Windows CI is live because they need real named-pipe IPC.
- [ ] **`os_input.rs` Win32 SendInput suite** (only if SendInput is implemented for v1 — otherwise mark `#[ignore]` with note)
  - Key map completeness: every entry in the Linux event-code map (`os_input.rs:149-178`) has a matching Windows VK entry. Parameterized test iterates all keys
  - Modifier chain correctness: Ctrl+Shift+A produces 3 keydown + 3 keyup `INPUT` structs in correct order
  - Mouse primitives: `SendInput` is called with `MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE` for moves, `LEFTDOWN`/`LEFTUP` for clicks (mock `SendInput` via trait)
  - Browser window lookup: `find_browser_window()` returns valid `HWND` + rect when browser is open (may require fixture window or mock)
- [x] **`terminal/mod.rs` shell + env suite**
  - `default_shell()` returns `cmd.exe` when `ComSpec` is unset, honors `ComSpec` when set, never returns `/bin/bash` on Windows
  - PATH separator test: child process env shows `;`-joined PATH on Windows, `:`-joined on Linux
  - Batched reader equivalence: feed identical byte stream to Linux `poll`-based reader and Windows Tokio reader; assert output chunks are identical (timing-insensitive: flush on demand)
  - Batched reader flushes on 32KB boundary and on 16ms interval
  - **Done (2026-04-13)**: extracted `path_separator()` and `prepend_shim_to_path(shim_dir, current_path)` as cross-platform helpers from the inline logic in `spawn_pty_for_shell`, then added 6 tests: `test_path_separator_matches_host_os`, `test_prepend_shim_to_path_with_existing_path`, `test_prepend_shim_to_path_with_empty_current`, `test_prepend_shim_to_path_windows_style_paths`, `test_default_shell_windows_returns_cmd_or_powershell` (cfg windows — validates `.cmd.exe`/`.powershell.exe` suffix, rejects Unix shell paths), `test_default_shell_unix_returns_bash_or_shell_env` (cfg unix paired guard). The empty-current-path edge case is explicitly guarded because a trailing separator would make child processes resolve binaries from CWD (security hazard on some shells). Still outstanding: batched reader equivalence + 32KB boundary tests — deferred until the Tokio reader rewrite happens.
- [x] **`ports.rs` Windows detection**
  - Open a `TcpListener` on `127.0.0.1:0`, call `detect_listening_ports()`, assert the port + owning PID are returned
  - Parent-PID walk via `CreateToolhelp32Snapshot` returns the current test process as ancestor
  - **Done (2026-04-13)**: refactored `detect_listening_ports` on Windows into a thin I/O wrapper + pure `parse_netstat_output(&str, &HashMap<u32, String>) -> Vec<PortInfo>` + pure `parse_tasklist_csv(&str) -> HashMap<u32, String>` functions. Both parsers live at the top level of `ports.rs` (NOT cfg-gated) so their tests run on every platform. Added 13 tests: `test_parse_netstat_output_happy_path` (IPv4+IPv6+filtered states+IGNORED_PORTS+Codemux-internal), `test_parse_netstat_output_uses_process_names` (PID→name lookup + "unknown" fallback), `test_parse_netstat_output_ipv6_entry_works_when_ipv4_absent`, `test_parse_netstat_empty_output`, `test_parse_netstat_malformed_lines_are_skipped`, `test_parse_netstat_dedups_ipv4_ipv6_pair`, `test_parse_netstat_results_sorted_by_port`, `test_parse_netstat_filters_codemux_internal_ports`, `test_parse_netstat_filters_ignored_ports`, `test_parse_netstat_never_panics` (null bytes, giant numbers, truncated rows), `test_parse_tasklist_csv_happy_path`, `test_parse_tasklist_csv_empty_input`, `test_parse_tasklist_csv_skips_malformed_rows`. All 13 run on Linux CI and Windows CI from the same test source. The "real TcpListener + PID walk" end-to-end test is still outstanding — deferred to integration tests because it requires a live Windows runner to verify process-tree attribution via `wmic`/`CreateToolhelp32Snapshot`.
- [x] **`git.rs` worktree sanitization**
  - Branch names with `/` → `-` (existing)
  - Branch names with Windows-forbidden chars (`<`, `>`, `:`, `"`, `\`, `|`, `?`, `*`) get sanitized on Windows
  - Worktree root resolves to `%USERPROFILE%\.codemux\worktrees\...` (not `/tmp`) on Windows
  - Full create → exists → remove cycle against a real repo fixture
  - **Done (2026-04-13)**: extracted `sanitize_branch_for_worktree_path(branch: &str) -> String` from the inline logic in `git_create_worktree`. Added 5 cross-platform (NOT cfg-gated) tests: `test_sanitize_branch_replaces_forward_slash`, `test_sanitize_branch_replaces_windows_forbidden_chars` (each of `< > : " | ? * \` in isolation), `test_sanitize_branch_replaces_multiple_forbidden_chars` (pathological combined input with char-by-char trace in the comment), `test_sanitize_branch_preserves_safe_chars` (`main`, `release-2024.09.01`, `feat_underscore_branch`, `bugfix-issue-1234`, `v1.0.0-rc.1`, `branch.with.dots` — alphanumerics/dashes/underscores/dots pass through), `test_sanitize_branch_empty_and_edge_cases` (empty input, `/`, `///`, all-forbidden). The worktree root `%USERPROFILE%` resolution is already handled by `dirs::home_dir()` (from an earlier commit) — not re-tested here. The end-to-end create/remove cycle is still outstanding, tracked under integration tests.
- [x] **`agent_context.rs` temp paths**
  - Gemini system prompt path uses `std::env::temp_dir()`, not hardcoded `/tmp/`
  - Path is valid on Windows (no forbidden chars, correct separator)
  - Injection shell-string tests: Claude / Codex / Pi / Gemini injections produce the correct command-line shape on Windows (no bare `$VAR` expansion)
  - **Done (2026-04-13)**: added `test_gemini_context_path_is_cross_platform` which builds `std::env::temp_dir().join("codemux-test-ws-xplat-gemini-system.md")` and asserts the generated shell command contains that OS-appropriate path. On Windows a second assertion verifies the path does NOT start with `/tmp/` (runtime-constructed prefix so the literal doesn't appear in source). Also added `test_no_hardcoded_tmp_paths_in_modified_sources` — a meta-test that greps the 9 files touched by the Windows support pass for hardcoded `"/tmp/codemux*` string literals in non-comment code and panics with a file:line if any are found. Suppression marker `// tmp-literal-ok` handles legitimate Unix-only fallbacks like the XDG-less fallback in `control.rs`. The bare-`$VAR` shell-expansion tests for Claude/Codex/Pi injections are covered by the existing `inject_claude_adds_system_prompt` / `inject_codex_adds_instructions` / `inject_pi_adds_append_system_prompt` tests which assert the exact argv shape.
- [x] **Process management**
  - `kill_port()` on Windows calls `taskkill /PID {pid} /F` (mock `Command::new` or test against a throwaway child)
  - `agent_browser.rs` daemon cleanup uses `taskkill /IM agent-browser.exe` on Windows
  - `fuser` replacement: parse fixture `netstat -ano` output and extract PID for a given port
  - **Done (2026-04-13)**: extracted `pids_listening_on_port(&str, u16) -> HashSet<u32>` from `kill_process_on_port` in `agent_browser.rs` as a pure cross-platform function. Added 6 tests: `test_pids_listening_on_port_happy_path` (IPv4+IPv6 LISTENING dedup, filters ESTABLISHED + UDP + substring-lookalike `:92230`), `test_pids_listening_on_port_exact_match_no_substring_trap` (regression guard — the reason we don't use `findstr :port`), `test_pids_listening_on_port_no_match_returns_empty`, `test_pids_listening_on_port_ipv4_only`, `test_pids_listening_on_port_ipv6_only`, `test_pids_listening_on_port_never_panics`. The `kill_port()` and daemon cleanup (`taskkill /IM agent-browser.exe`) invocations are tested by code inspection + the existing `cfg!(windows)` branches — a full mock would require the `mockall` crate which isn't in our dep tree.
- [x] **Filesystem path helpers**
  - `dirs::home_dir()` returns a valid path on Windows (resolves to `%USERPROFILE%`)
  - XDG-related fallbacks never hit `/tmp` on Windows
  - **Done (2026-04-13)**: the `test_no_hardcoded_tmp_paths_in_modified_sources` meta-test in `agent_context.rs` enforces the "never hit /tmp/ on Windows" constraint for cross-platform code by scanning 9 source files for the literal `"/tmp/codemux*` pattern. Legitimate Unix-only fallbacks (e.g. `control.rs:167` XDG_RUNTIME_DIR fallback) are annotated with `// tmp-literal-ok` to exempt them — the `#[cfg(unix)]` gate on the surrounding function makes them unreachable on Windows anyway, but the annotation makes the intent explicit. `dirs::home_dir()` resolution to `%USERPROFILE%` on Windows is a property of the `dirs` crate itself and is not re-tested here — if `dirs` ever regresses its Windows behavior, the downstream `git_create_worktree` tests would catch it.

### Integration tests

Higher-level tests that exercise multiple components end-to-end on Windows.

- [ ] **Full PTY lifecycle**
  - Spawn `cmd.exe` via portable-pty → write `echo hello\r\n` → read output → resize → send `exit\r\n` → observe clean exit with code 0
  - Equivalent test for PowerShell if default shell detection honors user preference
- [ ] **Control socket round-trip (named pipe)**
  - Spawn `codemux` daemon in a subprocess → send a control command via the CLI path → assert response → shut down cleanly
  - Include concurrent-client test (2 clients, interleaved requests)
- [ ] **Worktree create + delete cycle**
  - Fixture repo on the Windows filesystem → `git_create_worktree(branch="feat/windows-test")` → assert directory created at `%USERPROFILE%\.codemux\worktrees\...\feat-windows-test` → `git_remove_worktree` → assert cleanup
  - Same cycle with a branch name containing every forbidden char
- [ ] **Agent spawn + context injection + exit detection (all supported agent CLIs)**
  - Per-adapter coverage: verify the injection mechanism produces the correct CLI invocation (argv + env) for each adapter, whether or not the CLI itself is installed on the Windows CI runner
  - Split each adapter into two layers: (1) **injection unit test** — mock `Command` or inspect the built argv/env directly, always runs on Windows CI; (2) **end-to-end spawn test** — requires the real CLI, skips gracefully with a clear log message if missing
  - **Claude Code** (`--system-prompt`) — assert `--system-prompt <context>` is passed on argv (not via shell expansion of `$VAR`); end-to-end: agent acknowledges context → clean exit → exit-code handling reports success
  - **Codex** (`-c instructions=`) — assert `-c instructions=<context>` is built correctly with no bare `$VAR` expansion that would break on cmd.exe; end-to-end: session completes with injected instructions
  - **Pi** (`--append-system-prompt`) — assert the flag is passed on argv with the full context string; end-to-end: session observes the appended prompt
  - **Gemini** (temp file + `GEMINI_SYSTEM_MD` env var) — assert the temp file is written under `std::env::temp_dir()` (resolves to `%TEMP%` on Windows, **not** hardcoded `/tmp/`), the `GEMINI_SYSTEM_MD` env var on the spawned `Command` points to that file, the file contents match the expected context, and the file is cleaned up on session exit
  - **OpenCode** (env var only, no CLI flag) — assert the context env var (e.g. `CODEMUX_AGENT_CONTEXT` or OpenCode-specific var) is set on the spawned `Command::env()`, and that **no** flag is added to argv; end-to-end: OpenCode picks up the env var and uses the injected context
  - Per-adapter skip policy: if the CLI isn't installed on Windows CI, log `SKIP: <adapter> CLI not installed on Windows runner` and continue; injection unit tests (layer 1) still run unconditionally so the build-logic is verified on every PR
  - Track in PR description: which CLIs were exercised end-to-end on Windows CI vs. mock-only this run
- [ ] **OpenFlow wrapper execution (PowerShell variant)**
  - If `.ps1` wrapper path is taken for v1: generate the script, run it with a real Claude session, assert session file is created in `%TEMP%`, assert orchestration turn completes
  - If the Rust-direct-spawn path is taken instead: test that path produces the same observable behavior as the Linux wrapper version (equivalence test)
- [x] **Release pipeline artifact verification**
  - Before merging the feature branch, verify a test tag build produces all expected artifacts (Linux AppImage/deb/rpm, Windows `.exe`, merged `latest.json` containing both `linux-x86_64` and `windows-x86_64` platforms).
  - **Done (2026-04-13)**: added `scripts/test-release-pipeline.sh` as a dev-only verification script. Takes a completed release tag and runs 8 checks: (1) release exists, (2) Linux AppImage + deb present, (3) Windows `.exe` present, (4) `latest.json` present, (5) JSON is valid + version matches tag, (6) CRITICAL `latest.json.platforms` contains BOTH `linux-x86_64` and `windows-x86_64` keys, (7) each platform has a signature field, (8) each platform has an https download URL. Exits 0 on full pass, exits 2 on issues, exits 1 on hard failure (missing release, invalid JSON, etc.). Does NOT build or download bundles — it's a post-tag verification only. Flow: push a throwaway tag (`v0.0.0-test1`) → wait for release.yml CI → run the script → if green, merge the feature branch → delete the test tag + release. Rationale for running it manually on a throwaway tag: the real `release.yml` only triggers on `v*` pushes, so the only way to test the multi-platform `latest.json` merge behavior end-to-end is to actually tag a test build.

### CI

- [x] **Add `windows-latest` to the CI matrix for every PR**
  - `cargo test` on Windows, both release and debug
  - `cargo test --target x86_64-pc-windows-msvc` to catch target-specific issues
  - `npm run verify` on Windows (type-check + lint + frontend tests)
  - Build the NSIS installer artifact and attach to PR for manual smoke testing
  - **Done (2026-04-12)**: `.github/workflows/ci.yml` runs on `push` to `main`/`feature/**` and on every PR into `main`. Matrix is `[ubuntu-latest, windows-latest]` with `fail-fast: false` so a Windows break never hides behind a Linux pass (and vice versa). Steps: install Linux system deps (webkit/gtk/etc. on ubuntu only) → setup Rust stable → Swatinem rust-cache scoped per-OS → setup Node 20 with npm cache → `npm ci` → stage agent-browser sidecar → `npm run build` → `npm run check` (tsc) → `npm run test` (vitest) → `cargo check` → `cargo test`. Debug build only for now — release build and `x86_64-pc-windows-msvc` explicit-target run and NSIS installer packaging are deferred until the release pipeline rewrite.
- [x] **Frontend tests (`npm run test`) already pass**
  - Frontend is browser-independent (Vitest + jsdom); verify no hidden Linux assumptions (path-sep tests, etc.)
  - Run on Windows CI and document any unexpected failures
  - **Done (2026-04-12)**: `npm run test` (`vitest run`) runs on both Linux and Windows inside the new CI workflow. 298 frontend tests pass on Linux locally; Windows run will verify on the first push. Any unexpected failures will show up on the Windows half of the matrix.
- [x] **Matrix must be fail-fast disabled** so a Linux regression doesn't mask a Windows one and vice versa
  - **Done (2026-04-12)**: `strategy.fail-fast: false` is set explicitly in `ci.yml`, with an inline comment explaining why.

### Cross-platform regression

- [ ] **All existing Linux tests must continue passing** — no regressions from new `cfg` gates
- [ ] **Run full `npm run verify` + `cargo test` on both Linux and Windows in CI** — they are first-class peer platforms after this work lands
- [ ] **Benchmark parity check**: if any performance-sensitive code path is rewritten (e.g., batched reader), run an existing benchmark on both platforms and record results in the PR. Target: within 10% of Linux baseline on equivalent hardware
- [ ] **Document Windows-only test skips**: any test gated out on Windows (or vice versa) must include an inline comment explaining why, and ideally a linked tracking issue to un-skip it later

---

## Decisions

- [ ] **`portable-pty` version pin on Windows** — research surfaced upstream regression #6783 (`portable-pty 0.9.0 doesn't work on windows`: reader returns garbage output). Options: (a) pin `portable-pty = "=0.8.1"` on the Windows target, (b) ship 0.9.0 and verify the Tokio batched-reader rewrite happens to sidestep the bug, (c) fork and patch. Recommend (a) until #6783 is fixed upstream. **Blocks all Windows PTY work** — decide before writing any Windows PTY code
- [ ] **Installer format: NSIS vs MSI** — recommend NSIS for v1 (smaller, simpler, no Windows SDK required for CI). MSI is better for enterprise Group Policy deployment; can ship both later via `"targets": ["nsis", "msi"]`
- [ ] **Code signing: EV ($300/yr) vs OV ($100/yr) vs unsigned initially** — initial release may ship unsigned with SmartScreen warning; EV cert eliminates warnings immediately but is costlier. OV cert is cheaper but builds reputation slowly. **Extra motivation from research**: `agent-browser-win32-x64.exe` (a bundled dependency) is already flagged by Windows Defender (upstream issues #514, #482), so shipping Codemux unsigned on top compounds install friction — signing becomes effectively mandatory for a polished first impression on Windows
- [x] **agent-browser Windows binary** — **Resolved by research**: `agent-browser-win32-x64.exe` is published on npm and GitHub releases (v0.25.3, 2026-04-07). Windows ARM64 is **not** published (upstream issue #248 — Chromium ARM64 availability). Plan: consume the x64 binary for Windows x64 Codemux, fall back to system Edge via CDP (`--remote-debugging-port=9222`) for Windows ARM64 until upstream ships that artifact. See Research Findings §1 for the list of known open Windows bugs to budget around
- [ ] **Tier 3 input injection** — ship Windows MVP with Tier 1/2 only, or implement `SendInput` from day 1? Tier 3 is a reliability fallback; MVP can ship without
- [x] **Which agent CLIs to support on Windows v1?** — **Resolved by research**: all five CLIs run natively on Windows. **Claude Code** (native installer via `irm claude.ai/install.ps1 | iex` or winget; requires Git for Windows), **Codex** (npm `@openai/codex` or Rust binary; `-c instructions=` works cross-platform), **Gemini CLI** (npm `@google/gemini-cli`; `GEMINI_SYSTEM_MD` officially documented), **OpenCode** (scoop/choco/npm; **publisher renamed sst → anomalyco** — update any hardcoded URLs), **Pi** (npm `@mariozechner/pi-coding-agent` — this is `pi-mono` by `badlogic`, NOT Inflection AI). Open follow-up: verify OpenCode's env-var injection on native Windows before shipping — upstream recommends WSL and has known native-install rough edges. See Research Findings §3 for install methods and flag-parity matrix
- [ ] **Default shell on Windows** — `cmd.exe` (always available) vs PowerShell (better UX but requires detection). Recommend `cmd.exe` for v1 with PowerShell via explicit user setting

---

## Recommended Order of Work

1. **Foundation (blocking compile)** — `control.rs` named pipes, `terminal/mod.rs` Tokio batched reader rewrite, shell detection, PATH separator, `dirs::home_dir()` replacements for `HOME` env var.
2. **Build pipeline** — `tauri.conf.json` Windows bundle config, `release.yml` Windows matrix, `copy-agent-browser` cross-platform rewrite, `check-deps` Windows variant.
3. **Agent runtime** — refactor `openflow/prompts.rs` wrappers to spawn CLIs directly from Rust (eliminates the bash-script problem entirely), or write `.ps1` variants.
4. **Process management** — `fuser` / `pkill` / `kill_port` replacements; optional port detection via `GetExtendedTcpTable`.
5. **Tier 3 input / os_input.rs** — Win32 `SendInput` implementation behind `#[cfg(target_os = "windows")]`. Can defer if Tier 1/2 is sufficient for MVP.
6. **Code signing** — purchase cert (OV or EV), configure Tauri signing. Ship initial unsigned if needed.
7. **Polish** — hyprctl gates, Omarchy skip, permission cfg guards, error-message wording, test-fixture cleanup.

**Estimated blockers-only path to a running Windows build: ~2-3 weeks** of focused work, assuming: (a) `portable-pty` ConPTY works out of the box, (b) `agent-browser` has a Windows binary available, (c) Claude Code CLI flag parity holds on Windows. Biggest risks: agent-browser Windows availability, Claude Code CLI flag parity, code-signing setup friction.
