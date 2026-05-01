# Codemux Plan

- Purpose: Canonical roadmap and build order.
- Audience: Anyone deciding what to build next.
- Authority: Build-order document, not release-readiness proof.
- Update when: Major milestones, sequencing, or focus areas change.
- Read next: `docs/core/STATUS.md`, `docs/plans/browser.md`, `docs/plans/openflow.md`

## Planning Rules

- This file is about ordering and priorities, not current truth.
- Use `docs/core/STATUS.md` for repo reality and manual validation status.
- Keep subsystem execution details in `docs/plans/*`, not here.

## Roadmap At A Glance

1. Foundations landed in meaningful form: phases 0 through 9.6 established the workspace shell, multi-session terminals, pane management, notifications, browser prototype, CLI and socket control, indexing, and project memory.
2. Current focus: phases 10 through 15 are about hardening OpenFlow and getting the Linux + Windows release pipeline polished for daily use.
3. Parallel track: Phase 18 (Windows support) is **shipping**. The Windows foundation merged in commit `cc9b946` and shipped in `v0.1.20` and `v0.1.21`. `main` is currently 8 commits past `v0.1.21`, all of them post-release Windows fixes (PowerShell as default shell, portable-pty fork for `CREATE_NO_WINDOW` / `SW_HIDE`, scrollback flush backstop, editor detection rewrite, window controls on full-screen views, system-process port filtering). OpenFlow-on-Windows and Authenticode signing remain gated before a polished Windows v1. See `docs/plans/windows-support.md`.
4. Later: Phase 17 (macOS) has not been started.

## Ordered Phases

1. Phase 0: architecture baseline and MVP framing
2. Phase 1: stabilize prototype startup, errors, theming, and repo hygiene
3. Phase 2: define the real backend app domain model
4. Phase 3: deliver multi-terminal session management
5. Phase 4: build the workspace shell and sidebar
6. Phase 5: deliver splits and pane management
7. Phase 6: add notifications and attention workflows
8. Phase 7: ship a usable browser pane prototype toward MVP
9. Phase 8: expose browser automation for agents and tools
10. Phase 9: harden CLI and socket automation
11. Phase 9.5: add local-first codebase indexing
12. Phase 9.6: add portable project memory and handoff support
13. Phase 10: define OpenFlow core design and boundaries
14. Phase 11: build the OpenFlow runtime scaffold
15. Phase 12: integrate OpenFlow with Codemux workspace surfaces
16. Phase 13: harden the autonomous loop and intervention flow
17. Phase 14: improve quality, observability, and safety
18. Phase 15: complete Linux polish and release readiness
19. Phase 16: prepare cross-platform abstractions
20. Phase 17: add macOS support
21. Phase 18: add Windows support

## Immediate Priority Order

1. Cut the next release (`v0.1.22`) bundling the post-`v0.1.21` Windows fixes (PowerShell default shell, portable-pty fork, scrollback flush backstop, editor detection rewrite, window controls, system-process port filter) — the fixes are in the tree but every Windows user on `v0.1.21` is missing them
2. Harden OpenFlow reliability and intervention flow
3. Re-enable OpenFlow on Windows by rewriting the bash wrapper scripts in `openflow::prompts` (the only thing currently gating OpenFlow on Windows)
4. Add notification sound playback
5. Add memory drawer UI
6. Add context menus on pane headers (workspace rows, section groups, tabs, and the changes/ports sidebar panels already have them)
7. Windows Authenticode code signing — pick OV vs EV certificate once SmartScreen friction starts showing up in user reports
8. macOS support (Phase 17)

## Cross-Cutting Steps (Tracked Outside Phase Numbering)

The Phase numbering above (1-18) is the original product roadmap. Step-numbered work below is feature-scoped initiatives that span phases and have their own per-stage plan docs under `docs/plans/`. They land in parallel with the phase work.

- **Step 9 — Cross-provider MCP runtime (LANDED).** Plan: `docs/plans/step-9-mcp-servers.md` + `docs/plans/step-9-codex-mcp-spike.md`. Codex support deferred to Step 11.
- **Step 10 — End-to-end encrypted skills sync (LANDED).** Plan: `docs/plans/step-10-skills-sync.md`. Feature: `docs/features/skills-sync.md`.
- **Step 10.5 — Project-scoped skills sync (PLANNED, ~3-5 days).** Sync skills tied to specific git repos in addition to the user-global ones already shipping. Same skill name across projects with different content per repo is a real authoring pattern. Schema is additive: Stage 1's `user_skills` table reserves `scope` as plaintext; 10.5 adds `project_remote_url_hash TEXT NULL` (HMAC of normalized git remote URL — server sees "skills for repo-with-this-hash" but never which repo). Stages: 10.5.1 schema migration + URL normalization, 10.5.2 push pipeline filter, 10.5.3 pull pipeline filter, 10.5.4 Settings UI, 10.5.5 polish. Trickiest piece is URL canonicalization (`https://github.com/u/r` ↔ `git@github.com:u/r.git` ↔ `https://github.com/u/r.git` are the same repo).
- **Step 11 — Codex MCP support via HTTP gateway (PLANNED).** Spike: `docs/plans/step-9-codex-mcp-spike.md`. Codex doesn't expose an MCP host API, but `streamable HTTP MCP transport` landed stable in Codex (Sept 2025) + `config/mcpServer/reload` lets us hot-reload without bouncing the session. Plan: ship an HTTP MCP gateway in Codemux that Codex consumes as a single MCP entry, reusing the Step 9 registry. ~40-50% of Step 9 Stage 3's complexity.
- **Step 12 — OpenCode provider in chat GUI (DEFERRED).** Research: `docs/plans/step-12-opencode-research.md`. Plan: `docs/plans/step-12-opencode-implementation-plan.md`. Estimated 17-27 person-days (3.5-5.5 weeks elapsed) to add OpenCode as a third provider alongside Claude/Codex with a two-column provider-rail + flat-search model picker. Architectural mismatch — OpenCode is an HTTP server + SDK, not JSON-RPC over stdio, so it doesn't fit the existing `JsonRpcChild` sidecar shape and needs a new transport layer. Defer until: Step 10 fully dogfooded (skills sync proven in real use); Step 10.5 shipped (project-scoped sync); Step 11 shipped (Codex MCP via HTTP gateway); user demand confirmed via real-world signal. Deferral rationale: each preceding step widens the user base for the next, and Step 12 is the largest commitment with the lowest validated demand. The Stage-3.5 partial-ship path explored in the plan doc was rejected — half-shipping without the picker rebuild leaves users typing model slugs into a text field, which is a TODO with a maintenance burden, not a feature.

## Recently Completed

- **Step 10 — End-to-end encrypted skills sync (Stages 1-6, shipping)**: skills under `~/.codemux/skills/` and any other recognized provider's user path now sync end-to-end-encrypted across every device the user signs into. XChaCha20-Poly1305 per blob, key derived from `(password, email)` via the cross-product `codemux-api-*` HKDF protocol (byte-identical with Vexis, pinned in CI), persisted machine-bound at `~/.local/share/codemux/sync-key.enc`. Five `/api/skills` routes deployed to production, plus a custom `POST /api/auth/set-password` route to work around an upstream Better Auth bug. Settings → Account → Sync surface drives status + Export/Import/Forgot-password. Live programmatic smoke (`examples/stage5_smoke.rs`) verified against `api.codemux.org`. Plan + per-stage history at `docs/plans/step-10-skills-sync.md`; current behavior at `docs/features/skills-sync.md`; manual UI smoke checklist at `docs/plans/step-10-ui-smoke-checklist.md`.
- Windows post-release hardening pass (commits between `v0.1.21` and current `main`): forked `portable-pty` to add `CREATE_NO_WINDOW` + `STARTF_USESHOWWINDOW + SW_HIDE` in `psuedocon.rs` (kills the visible `cmd.exe` flash on every PTY spawn), switched `default_shell()` to prefer PowerShell (`pwsh` → `powershell` → `COMSPEC` → `cmd.exe`), taught `agent_context::inject_agent_context` to emit PowerShell `$env:VAR` syntax for Claude / Codex / Pi / Gemini presets, rewrote `find_editors()` to use the `which::which()` Rust crate plus `%LOCALAPPDATA%\Programs` / `%ProgramFiles%` fallbacks for VS Code / Cursor / VSCodium / Zed, extracted `<WindowChrome />` so login / settings / empty-state screens have minimize/maximize/close buttons on Windows, extended the close-handler scrollback flush timeout from 3s to 10s on Windows with a backend `flush_cache_to_disk` backstop for the cache miss case, added a Windows system-process name filter (`svchost.exe`, `System`, `lsass.exe`, etc.) so port detection doesn't surface 16+ kernel ports, routed preset failures through the sonner toast wrapper, and switched Windows preset commands to `\r` line terminators
- Windows support foundation released in `v0.1.20` and `v0.1.21`: the `cc9b946` merge landed first, then `v0.1.20` / `v0.1.21` shipped to users with cfg gates, named-pipe control socket, Windows port detection, NSIS installer build, multi-platform `release.yml` matrix, cross-platform `latest.json` via tauri-action merge, and 38 Windows-specific tests
- PTY child process tree cleanup on session close: single `killpg(pid, SIGKILL)` via a central `terminate_pty_session` helper called from every close path. Closes the TOCTOU window that previous SIGTERM→200ms→SIGKILL dance left exposed to PID recycling. Leaked ~20 GiB/day of zombie processes (Claude CLI, MCP servers, rust-analyzer) before the fix; now every close path tears down the whole group. `impl Drop for SessionRuntime` is a safety net that kills the group with a warning if any future refactor skips the normal close path.
- Session persistence: terminal scrollback save/restore and adapter-based resume
- GitHub issue integration (link issues to workspaces, issue picker, auto-branch naming)
- Browser wait conditions, JS evaluation, and CSS style inspection (MCP tools, 26→29)
- Custom keybind editor in Settings
- Agent context injection for preset launches (Claude, Codex, Pi, Gemini)
- Auto-update system (AppImage in-app update, toast notification)
- Built-in file editor with CodeMirror, syntax highlighting, and markdown preview
- AI merge conflict resolver with temp-branch safety model
- MCP server for agent self-orchestration (29 tools via JSON-RPC 2.0)
- Settings panel (keyboard shortcuts, appearance, project scripts)
- Auth system (GitHub OAuth, email/password, email verification)
- Synced settings (per-user server-synced with offline cache)
- Claude CLI adapter for OpenFlow
