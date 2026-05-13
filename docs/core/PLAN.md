# Codemux Plan

- Purpose: Canonical roadmap and build order.
- Audience: Anyone deciding what to build next.
- Authority: Build-order document, not release-readiness proof.
- Update when: Major milestones, sequencing, or focus areas change.
- Read next: `docs/core/STATUS.md`, `docs/plans/openflow.md`, `docs/plans/windows-support.md`

## Planning Rules

- This file is about ordering and priorities, not current truth.
- Use `docs/core/STATUS.md` for repo reality and manual validation status.
- Keep subsystem execution details in `docs/plans/*`, not here.

## Roadmap At A Glance

1. Foundations landed in meaningful form: phases 0 through 9.6 established the workspace shell, multi-session terminals, pane management, notifications, browser prototype, CLI and socket control, indexing, and project memory.
2. Steps 6 through 13 (cross-cutting agent-chat work) landed on `feature/agent-chat` and merged to `main` behind the Step 13 Beta Features toggle. The full chat stack — Claude / Codex / OpenCode providers, attachments, slash commands, skills sync, MCP host runtime — ships in every release since `v0.2.0` but is OFF by default. Users opt in via Settings → Beta Features.
3. Current focus: phases 10 through 15 (OpenFlow hardening + Linux/Windows release polish), plus moving the agent-chat surface from opt-in Beta to default-on.
4. Parallel track: Phase 18 (Windows support) is **shipping** in every release. Foundation merged in `cc9b946` and shipped in `v0.1.20`/`v0.1.21`; the hardening pass (PowerShell as default shell, portable-pty fork, scrollback flush backstop, editor detection rewrite, window controls, system-process port filtering, agent-browser argv + Chromium auto-detect, control-pipe retry, Tier-3 SendInput) is on `main` and shipping. OpenFlow-on-Windows and Authenticode signing remain gated before a polished Windows v1.
5. Later: Phase 17 (macOS) has not been started.

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

1. Harden OpenFlow reliability and intervention flow (the largest remaining stability gap)
2. Re-enable OpenFlow on Windows by rewriting the bash wrapper scripts in `openflow::prompts`
3. Move the agent-chat surface from Beta-gated to default-on once dogfooding settles
4. Add memory drawer UI
5. Add context menus on pane headers (workspace rows, section groups, tabs, and the changes/ports sidebar panels already have them)
6. Windows Authenticode code signing — pick OV vs EV certificate once SmartScreen friction starts showing up in user reports
7. macOS support (Phase 17)

## Cross-Cutting Steps (Tracked Outside Phase Numbering)

The Phase numbering above (1-18) is the original product roadmap. Step-numbered work below is feature-scoped initiatives that span phases and have their own per-stage plan docs under `docs/plans/`. They land in parallel with the phase work.

- **Step 6–7 — Agent-chat pane + provider scaffolding (LANDED).** AgentProvider trait + Codex/Claude adapters + Tauri command surface + chat pane UI with streaming + approvals + mode pills + permission-mode restart. Behavior doc: `docs/features/agent-chat.md`.
- **Step 8 — Attachments + context system (LANDED).** Files, folders, GitHub issues + PRs, images via `+` button + `@` mentions, paste, drop. Plan: `docs/plans/step-8-attachments.md`.
- **Step 9 — Cross-provider MCP runtime (LANDED).** Plan: `docs/plans/step-9-mcp-servers.md` + `docs/plans/step-9-codex-mcp-spike.md`. Codex MCP support deferred to Step 11.
- **Step 10 — End-to-end encrypted skills sync (LANDED).** Plan: `docs/plans/step-10-skills-sync.md`. Feature: `docs/features/skills-sync.md`.
- **Step 10.5 — Project-scoped skills sync (PLANNED, ~3-5 days).** Sync skills tied to specific git repos in addition to the user-global ones already shipping. Schema is additive: Stage 1's `user_skills` table already reserves `scope` as plaintext; 10.5 adds `project_remote_url_hash TEXT NULL` (HMAC of normalized git remote URL — server sees "skills for repo-with-this-hash" but never which repo). Trickiest piece is URL canonicalization (`https://github.com/u/r` ↔ `git@github.com:u/r.git` ↔ `https://github.com/u/r.git` are the same repo).
- **Step 11 — Codex MCP support via HTTP gateway (PLANNED).** Spike: `docs/plans/step-9-codex-mcp-spike.md`. Codex doesn't expose an MCP host API, but streamable HTTP MCP transport landed stable in Codex (Sept 2025) + `config/mcpServer/reload` lets us hot-reload without bouncing the session. Plan: ship an HTTP MCP gateway in Codemux that Codex consumes as a single MCP entry, reusing the Step 9 registry. ~40-50% of Step 9 Stage 3's complexity.
- **Step 12 — Multi-provider chat (Claude + Codex + OpenCode) (LANDED).** Feature doc: `docs/features/multi-provider-chat.md`. Research: `docs/plans/step-12-opencode-research.md`. Plan + per-stage history: `docs/plans/step-12-opencode-implementation-plan.md`. Operator UI smoke: `docs/plans/step-12-ui-smoke-checklist.md`. Stages 1-9 shipped (Stage 5 explicitly deferred to v2). Deferred follow-ups: multi-instance per provider, picker keyboard shortcuts (`Ctrl+1..9` collides with workspace switching), OpenFlow capabilities convergence.
- **Step 13 — Agent Chat Beta toggle (LANDED).** Single Settings toggle "Beta Features" that controls every Step 6–12 GUI surface so the merge to `main` ships with the new behaviour OFF by default and the legacy behaviour preserved. Unifies `enable_agent_chat` and `enable_lazy_workspace_creation` flags. Research: `docs/plans/step-13-beta-toggle-research.md`. Smoke checklist: `docs/plans/step-13-ui-smoke-checklist.md`. Settings UI in `src/components/settings/beta-features-section.tsx`. Plain-quit on toggle off (no auto-restart) keeps user data intact.

## Recently Completed

- **Browser stream stability fix (LANDED).** Unified port keying around `workspace_id`, PID-tracked daemons (was port-tracked), atomic teardown, symmetric `TcpListener::bind` bind probe, reactive `stream_url` reconnect on the frontend. Eliminates the silent stream failure that appeared after multiple concurrent worktrees used the browser. Plan archived at `docs/archive/browser-stream-fix.md`.
- **Browser viewport presets.** `codemux browser viewport <mobile|tablet|desktop|WxH|reset>` resizes the actual viewport via CDP so CSS media queries fire and screenshots capture at the simulated dimensions. MCP exposes `browser_viewport` + `browser_viewport_presets`.
- **Performance pass** (`v0.2.3`–`v0.2.4`): high-frequency app-state emits coalesced into 16ms windows, `transition-all` scoped, markdown view + workspace-tied components stop re-rendering on every backend tick, workspace-switch mount-time IPC roundtrips cut, editor file read + language module import parallelised, worktree-include listener no longer re-attaches every tick, primitive fingerprint for `ensure-draft-when-empty` effect, all git/gh shell-outs moved off the GTK main thread.
- **Review tab unfreeze** on repos with thousands of PRs (paginated fetch).
- **Per-workspace display isolation** for agent-spawned GUI apps (X11/Wayland sandbox). Opt-in for human persona, default-on for agent persona; the human-default revert restored DISPLAY inheritance for the GUI flows where users actually paste images via Ctrl+V.
- **Bitwarden-style password derivation** ported for cross-product login parity with Vexis (Argon2id + HKDF, pinned in CI).
- **Stability**: stopped leaking agent-chat sidecars on workspace/tab/pane close; throttle reattach replay so multi-MB `pending_output` can't freeze the renderer; cap `pending_output` by bytes (256 MiB) rather than chunks.
- **Windows post-release hardening** between `v0.1.21` and `v0.2.0`: forked `portable-pty` to add `STARTF_USESHOWWINDOW + SW_HIDE`, switched `default_shell()` to prefer PowerShell, taught `agent_context::inject_agent_context` to emit `$env:VAR` syntax, rewrote `find_editors()`, extracted `<WindowChrome />`, extended scrollback flush timeout to 10s + added `flush_cache_to_disk` backstop, Windows system-process port filter, sonner toast for preset failures, `\r` preset terminators, Tier-3 SendInput input injection, Claude Code hooks on Windows, agent-browser argv fix + ERROR_PIPE_BUSY retry, Edge auto-detect, `resolve_binary` Windows discovery branch.
- **PTY child process tree cleanup** on session close: single `killpg(pid, SIGKILL)` via a central `terminate_pty_session` helper. Closes the TOCTOU window the old SIGTERM→200ms→SIGKILL dance exposed to PID recycling.
- Session persistence: terminal scrollback save/restore and adapter-based resume
- GitHub issue integration (link to workspaces, picker, auto-branch naming)
- Browser wait conditions, JS evaluation, CSS style inspection (MCP tools)
- Custom keybind editor in Settings
- Auto-update system (AppImage / NSIS in-app update, toast notification)
- Built-in file editor with CodeMirror, syntax highlighting, markdown preview
- AI merge conflict resolver with temp-branch safety model
- MCP server for agent self-orchestration (31 tools via JSON-RPC 2.0)
- Settings panel (keyboard shortcuts, appearance, project scripts, beta features, sync, skills, MCP, permissions)
- Auth system (GitHub OAuth, email/password, email verification, encrypted token storage)
- Synced settings (per-user server-synced with offline cache)
- Claude CLI adapter for OpenFlow
