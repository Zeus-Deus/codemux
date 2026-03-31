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
3. Later: phases 16 through 18 cover cross-platform preparation, then macOS and Windows support.

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

1. Harden OpenFlow reliability and intervention flow
2. Add tasks system (Linear/GitHub issue integration)
3. Improve browser pane fidelity (wait conditions, DOM inspection, DevTools)
4. Add notification sound playback
5. Add memory drawer UI
6. Add context menus on workspace rows, sections, and panes
7. Add custom keybind editor
8. Linux release packaging and polish
9. Cross-platform preparation (macOS, Windows)

## Recently Completed

- Built-in file editor with CodeMirror, syntax highlighting, and markdown preview
- AI merge conflict resolver with temp-branch safety model
- MCP server for agent self-orchestration (26 tools via JSON-RPC 2.0)
- Settings panel (keyboard shortcuts, appearance, project scripts)
- Auth system (GitHub OAuth, email/password, email verification)
- Synced settings (per-user server-synced with offline cache)
- Claude CLI adapter for OpenFlow
