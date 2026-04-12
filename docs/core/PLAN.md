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
2. Current focus: phases 10 through 15 are about turning that foundation into a trustworthy Linux MVP while hardening OpenFlow.
3. Parallel track: Phase 18 (Windows support) foundation work is in progress on the `feature/windows-support` branch. The release pipeline, matrix CI, `cfg`-gated code paths, and Windows-specific tests have landed and been verified end-to-end against a throwaway release tag. OpenFlow and Authenticode signing remain gated before a real Windows v1. See `docs/plans/windows-support.md`.
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

1. Merge the Windows support foundation branch to main (blocked on Authenticode signing decision and final OpenFlow disable verification on a live Windows install)
2. Harden OpenFlow reliability and intervention flow
3. Add notification sound playback
4. Add memory drawer UI
5. Add context menus on workspace rows, sections, and panes
6. Linux release packaging and polish
7. macOS support (Phase 17)

## Recently Completed

- Windows support foundation: cfg gates, named-pipe control socket, Windows port detection, NSIS installer build, multi-platform release.yml matrix, cross-platform `latest.json` via tauri-action merge, 38 new tests for Windows code paths, verification via throwaway test tag against the live release pipeline (see `docs/plans/windows-support.md`)
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
