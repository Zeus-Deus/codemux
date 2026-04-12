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

- [ ] **Verify `portable-pty` ConPTY feature is enabled** — `src-tauri/Cargo.toml`
  - Confirm no feature flag or default disables Windows support. All downstream PTY items depend on this.
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
- [ ] **`src-tauri/src/terminal/mod.rs:368-412` — `batched_reader_loop(...)`**
  - Batches PTY output every 16ms or 32KB using `poll()`
  - Rewrite with `tokio::select!` on read future + `tokio::time::interval(Duration::from_millis(16))`; preserve 32KB flush threshold and batching semantics

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
- [ ] **`src-tauri/src/commands/mod.rs:274-292` — `kill_port(port)` shells out to `kill -9 {pid}`**
  - Kills process listening on a port
  - Windows branch: `taskkill /PID {pid} /F`, or use `windows` crate `TerminateProcess`
- [ ] **`src-tauri/src/agent_browser.rs:89` — `pkill -f agent-browser` for stale daemon cleanup**
  - Kills zombie agent-browser daemons from previous runs
  - Windows: `taskkill /IM agent-browser.exe /F`; `#[cfg(target_os = "windows")]` guard
- [ ] **`src-tauri/src/agent_browser.rs:470, 619, 670` — `fuser -k {port}/tcp` for port reclamation**
  - Frees ports stuck after crash
  - Windows: parse `netstat -ano` for PID + `taskkill /PID`, or use `GetExtendedTcpTable` from `windows` crate

### Browser Automation / Tier 3 Input Injection

> **Scope:** `src-tauri/src/os_input.rs` is entirely Linux-specific. Every item below needs a `#[cfg(target_os = "windows")]` twin implementation using Win32 `SendInput` (via the `windows` crate or `enigo`).

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

- [ ] **`src-tauri/src/lib.rs:369-377` — startup `hyprctl keyword windowrule float...` for xdg-desktop-portal-gtk dialogs**
  - Hyprland-specific workaround for tiled file dialogs
  - Gate with `#[cfg(target_os = "linux")]`; Windows native dialogs don't need this

### Filesystem / IPC

- [ ] **`src-tauri/src/control.rs:11` — `use tokio::net::{UnixListener, UnixStream}`**
  - IPC transport for codemux control socket
  - `#[cfg(unix)]`-gate; Windows branch uses `tokio::net::windows::named_pipe::{NamedPipeServer, NamedPipeClient}`
- [ ] **`src-tauri/src/control.rs:30-76` — `control_socket_path()` resolves `$XDG_RUNTIME_DIR/codemux.sock` or `/tmp/codemux-{uid}/codemux.sock`**
  - Socket path resolution with Unix fallback
  - Windows: return `\\.\pipe\codemux-{username}` named pipe path; use `whoami::username()` or `%USERNAME%` env var
- [ ] **`src-tauri/src/control.rs:36` — `let uid = unsafe { libc::getuid() }`**
  - Unix UID lookup for socket dir naming
  - Windows: `whoami::username()` or `GetUserNameW` via `windows` crate
- [ ] **`src-tauri/src/control.rs:127` — `UnixStream::connect()` liveness probe on existing socket**
  - Checks if daemon already owns the socket
  - Windows: `NamedPipeClient::connect()` probe
- [ ] **`src-tauri/src/control.rs:178-199` — `UnixListener::bind(&socket_path)` for IPC server**
  - Main control server listen
  - Windows: `ServerOptions::new().create(pipe_name)` from `tokio::net::windows::named_pipe`
- [ ] **`src-tauri/src/main.rs:5, 199` — `use std::os::unix::net::UnixStream` for CLI daemon detection**
  - `codemux` CLI uses this to detect a running daemon
  - Windows: mirror the named-pipe probe from control.rs
- [ ] **`src-tauri/src/git.rs:1247-1324, 1261` — `git_create_worktree()` uses `env::var("HOME")` with `/tmp` fallback**
  - Worktree root path: `$HOME/.codemux/worktrees/{repo}/{branch}`
  - Replace with `dirs::home_dir()` (resolves to `%USERPROFILE%` on Windows); drop `/tmp` fallback
- [ ] **`src-tauri/src/agent_context.rs:102` — hardcoded `/tmp/codemux-{workspace_id}-gemini-system.md`**
  - Gemini system prompt temp file
  - Use `std::env::temp_dir().join(format!("codemux-{}-gemini-system.md", workspace_id))`

### Shell & Environment

- [ ] **`src-tauri/src/terminal/mod.rs:690-692` — PATH prepend uses `:` separator**
  - Prepends codemux CLI shim dir to child PATH
  - `if cfg!(windows) { ";" } else { ":" }`, or use `std::env::join_paths`
- [ ] **`src-tauri/src/terminal/mod.rs:414-419` — `default_shell()` reads `SHELL` → `/bin/bash` fallback**
  - User shell detection for terminal panes
  - Windows branch: `env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into())`; future: detect PowerShell via `HKLM\SOFTWARE\Microsoft\PowerShell`
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

- [ ] **`src-tauri/tauri.conf.json:31` — `bundle.targets = "all"` but only `linux` section configured**
  - No Windows installer config present
  - Add `"windows": { "nsis": { "languages": ["English"], "displayLanguageSelector": false } }`. Recommend NSIS over MSI for v1 — smaller, simpler, no Windows SDK required
- [ ] **`src-tauri/tauri.conf.json:6-9` — `beforeBuildCommand` / `beforeDevCommand` invoke `bash scripts/copy-agent-browser.sh`**
  - Hard-codes `bash`
  - Replace with Node.js script (`scripts/copy-agent-browser.mjs`) for true cross-platform, or conditionally invoke a `.ps1` variant
- [ ] **`.github/workflows/release.yml:10` — `runs-on: ubuntu-22.04` only; installs Linux-only deps (`libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `librsvg2-dev`, `libfuse2`)**
  - No Windows build in CI
  - Add matrix: `os: [ubuntu-22.04, windows-latest, macos-latest]`. Windows uses MSVC toolchain (pre-installed on `windows-latest`) and WebView2 runtime (pre-installed on recent Windows images)
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

---

## Degraded (feature broken, app runs)

### Process / Port Detection

- [ ] **`src-tauri/src/ports.rs:38-63` — `detect_listening_ports()` parses `/proc/net/tcp` and `/proc/net/tcp6`**
  - Port discovery for UI
  - Windows: `GetExtendedTcpTable()` via `windows` crate, or accept empty list on Windows (users type port manually)
- [ ] **`src-tauri/src/ports.rs:120` — `/proc/*/fd/` symlink scan to map sockets → PIDs**
  - Owner process attribution for detected ports
  - `GetExtendedTcpTable` returns PID directly; no fd scan needed
- [ ] **`src-tauri/src/ports.rs:156-166` — `read_ppid()` reads `/proc/{pid}/stat` for parent PID walk**
  - Walks up process tree to attribute ports to workspaces
  - Windows: `CreateToolhelp32Snapshot` + `Process32First/Next`
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

- [ ] **`src-tauri/src/control.rs:40-70` — `MetadataExt::uid()` ownership check + `Permissions::from_mode(0o700)` on fallback socket dir**
  - Security: ensures only current user can connect
  - Windows named pipes inherit user security context automatically; skip this check on Windows
- [ ] **`src-tauri/src/git.rs:1260` — branch-name sanitization replaces only `/` with `-`**
  - Used to build worktree directory name
  - Windows forbids `< > : " \ | ? *` in filenames. Expand sanitizer: `branch.replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '_', "-")`
- [ ] **`src-tauri/src/config/mod.rs:71` — reads `~/.config/omarchy/current/theme/colors.toml` for Omarchy theme integration**
  - Omarchy is a Linux/Hyprland theming tool
  - Skip entirely on Windows; theme falls back to default
- [ ] **`src-tauri/src/hooks.rs:317-323` — `#[cfg(unix)] set_permissions(Permissions::from_mode(0o755))` on hook script**
  - Makes hook script executable on Unix
  - Already cfg-gated; Windows uses file extension (`.bat`/`.ps1`) for executability. No action beyond ensuring the script itself is `.bat`/`.ps1` (see Shell section)
- [ ] **`src-tauri/src/openflow/prompts.rs:411-414, 606-609` — `0o755` on generated wrapper scripts**
  - OpenFlow + Claude wrapper scripts
  - Use `.ps1` extension on Windows (see Shell section); drop mode-bit setting inside `#[cfg(windows)]`
- [ ] **`src-tauri/src/terminal/mod.rs:421-437` — `#[cfg(unix)] ensure_openflow_cli_shims()` creates `/tmp/codemux-openflow-shims/` with 0o755**
  - CLI shim installation for PATH injection
  - Windows: create `.bat` shim in `%LOCALAPPDATA%\Codemux\shims`, or skip if PATH injection isn't needed there

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

- [ ] **`src-tauri/Cargo.toml:26` — `tauri-plugin-dialog` with `xdg-portal` feature enabled unconditionally**
  - Linux file dialog via xdg-portal
  - Gate feature to Linux: `[target.'cfg(unix)'.dependencies] tauri-plugin-dialog = { version = "2", features = ["xdg-portal"] }`; Windows uses native dialogs automatically
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
- [ ] **`src-tauri/src/commands/workspace.rs:771-773` — `hyprctl dispatch focuswindow class:com.codemux.app` after notification**
  - Hyprland window focus
  - Gate with `#[cfg(target_os = "linux")]`. Tauri's `window.request_user_attention()` + `window.set_focus()` already handles Windows transparently
- [ ] **`src-tauri/src/diagnostics.rs:34` — tries `$XDG_RUNTIME_DIR` for diagnostics log path**
  - Linux runtime dir
  - Add Windows fallback: `%LOCALAPPDATA%\Codemux\logs`
- [ ] **`codemux.desktop` + `src-tauri/tauri.conf.json:37-39` — bundles `.desktop` file to `/usr/share/applications/` for Linux deb**
  - Linux desktop entry
  - Already Linux-conditional; NSIS/MSI bundler will handle Windows Start Menu shortcut via tauri.conf.json
- [ ] **`src-tauri/src/commands/browser.rs:158-161` — `~/.agent-browser` dir with `/tmp` fallback**
  - Agent browser state dir
  - Use `std::env::temp_dir()` for fallback; home dir is reliably available on Windows via `dirs::home_dir()`
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

- [ ] **`control.rs` named pipe suite**
  - Bind server to pipe, spawn client, connect, send command, receive response
  - Liveness probe: second `control_socket_path()` call detects existing pipe
  - Stale pipe cleanup: dead server leaves pipe that new server can reclaim
  - Permission isolation: verify pipe is only accessible to current user SID (named pipes inherit user security; assert the ACL)
  - Connection timeout + error paths
- [ ] **`os_input.rs` Win32 SendInput suite** (only if SendInput is implemented for v1 — otherwise mark `#[ignore]` with note)
  - Key map completeness: every entry in the Linux event-code map (`os_input.rs:149-178`) has a matching Windows VK entry. Parameterized test iterates all keys
  - Modifier chain correctness: Ctrl+Shift+A produces 3 keydown + 3 keyup `INPUT` structs in correct order
  - Mouse primitives: `SendInput` is called with `MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE` for moves, `LEFTDOWN`/`LEFTUP` for clicks (mock `SendInput` via trait)
  - Browser window lookup: `find_browser_window()` returns valid `HWND` + rect when browser is open (may require fixture window or mock)
- [ ] **`terminal/mod.rs` shell + env suite**
  - `default_shell()` returns `cmd.exe` when `ComSpec` is unset, honors `ComSpec` when set, never returns `/bin/bash` on Windows
  - PATH separator test: child process env shows `;`-joined PATH on Windows, `:`-joined on Linux
  - Batched reader equivalence: feed identical byte stream to Linux `poll`-based reader and Windows Tokio reader; assert output chunks are identical (timing-insensitive: flush on demand)
  - Batched reader flushes on 32KB boundary and on 16ms interval
- [ ] **`ports.rs` Windows detection** (only if Win32 port detection is implemented)
  - Open a `TcpListener` on `127.0.0.1:0`, call `detect_listening_ports()`, assert the port + owning PID are returned
  - Parent-PID walk via `CreateToolhelp32Snapshot` returns the current test process as ancestor
- [ ] **`git.rs` worktree sanitization**
  - Branch names with `/` → `-` (existing)
  - Branch names with Windows-forbidden chars (`<`, `>`, `:`, `"`, `\`, `|`, `?`, `*`) get sanitized on Windows
  - Worktree root resolves to `%USERPROFILE%\.codemux\worktrees\...` (not `/tmp`) on Windows
  - Full create → exists → remove cycle against a real repo fixture
- [ ] **`agent_context.rs` temp paths**
  - Gemini system prompt path uses `std::env::temp_dir()`, not hardcoded `/tmp/`
  - Path is valid on Windows (no forbidden chars, correct separator)
  - Injection shell-string tests: Claude / Codex / Pi / Gemini injections produce the correct command-line shape on Windows (no bare `$VAR` expansion)
- [ ] **Process management**
  - `kill_port()` on Windows calls `taskkill /PID {pid} /F` (mock `Command::new` or test against a throwaway child)
  - `agent_browser.rs` daemon cleanup uses `taskkill /IM agent-browser.exe` on Windows
  - `fuser` replacement: parse fixture `netstat -ano` output and extract PID for a given port
- [ ] **Filesystem path helpers**
  - `dirs::home_dir()` returns a valid path on Windows (resolves to `%USERPROFILE%`)
  - XDG-related fallbacks never hit `/tmp` on Windows

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

### CI

- [ ] **Add `windows-latest` to the CI matrix for every PR**
  - `cargo test` on Windows, both release and debug
  - `cargo test --target x86_64-pc-windows-msvc` to catch target-specific issues
  - `npm run verify` on Windows (type-check + lint + frontend tests)
  - Build the NSIS installer artifact and attach to PR for manual smoke testing
- [ ] **Frontend tests (`npm run test`) already pass**
  - Frontend is browser-independent (Vitest + jsdom); verify no hidden Linux assumptions (path-sep tests, etc.)
  - Run on Windows CI and document any unexpected failures
- [ ] **Matrix must be fail-fast disabled** so a Linux regression doesn't mask a Windows one and vice versa

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
