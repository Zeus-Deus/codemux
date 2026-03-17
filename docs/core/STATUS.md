# Codemux Status

- Purpose: Canonical reality snapshot for the repo.
- Audience: Anyone deciding what is actually true today.
- Authority: Current implementation truth and near-term validation priorities.
- Update when: Behavior, constraints, or known gaps change.
- Read next: `docs/core/PLAN.md`, `docs/core/TESTING.md`

## Current Headline

Codemux is not ship-ready yet. The workspace shell and local automation foundation are real enough to keep using, but browser polish, OpenFlow reliability, and Linux release validation are still in progress.

## Solid Enough To Treat As Real Surface

- workspace shell and sidebar
- multi-session terminals
- pane splits, resizing, close, swap, and restore
- notifications and attention badges
- local project memory and lexical indexing
- local CLI and socket control basics

## Partial Or Prototype-Level

- browser pane: screenshot-driven Chromium via `agent-browser`, shared with CLI commands, not a native embedded Tauri webview
- browser automation: usable through explicit CLI commands, but fidelity and manual validation still need work
- OpenFlow: real agent PTYs, shared communication logs, and orchestration UI exist, but large-run reliability and intervention flow still need hardening
- browser console log capture is not yet a complete live stream from the displayed pane
- notification sound toggle exists, but actual sound playback is not implemented

## Known Constraints

- notification click-to-focus on Wayland and mako still needs deeper D-Bus or native handling
- control socket is local-user only and currently unauthenticated
- the current browser pane is a working prototype, not final Linux MVP proof

## Current Validation Priorities

- core Linux daily-driver workflows end to end
- browser toolbar, input, and manual interaction passes
- CLI and socket, memory, and index flows
- OpenFlow run creation, visibility, control actions, and 15-20 agent reliability

## Read This With

- `docs/core/PLAN.md` for build order
- `docs/core/TESTING.md` for verification policy
- `docs/features/browser.md` or `docs/features/openflow.md` for subsystem detail
