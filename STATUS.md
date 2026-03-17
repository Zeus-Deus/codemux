# Codemux Current Status

This file is the reality check for the repo.

`PLAN.md` is the roadmap and build order.
`STATUS.md` is the current source of truth for what is implemented, what is only partial, and what still needs manual validation before calling the app ship-ready.

## Current Headline

Codemux is not ship-ready yet.

What is solid enough to keep testing as real product surface:

- workspace shell and sidebar
- multi-session terminals
- pane splits, resizing, close, and swap
- notifications and attention badges
- local memory and indexing flows
- local CLI/socket control basics

What is still prototype-level or incomplete:

- browser pane now works using agent-browser (npm package via npx), shares session with CLI commands
- browser automation: now uses agent-browser, agents can control via CLI commands
- OpenFlow runtime/UI now run real agent PTYs and shared communication logs, but orchestration reliability and dev-time stability are still prototype-level
- control socket is local-user only and currently unauthenticated

Because of that, the first Linux MVP should still be considered in progress.

## Status Meanings

- Implemented: code exists and is wired into the app
- Partial: feature exists in some form but is not yet trustworthy as a finished product capability
- Verified: manually tested recently enough to trust for release decisions

## What Looks Real Today

- Workspaces: create, rename, activate, close
- Pane tree: split right, split down, resize, close, swap, restore saved layout
- Terminals: multiple sessions, lifecycle state, resize, restart, reconnect on remount
- Sidebar: workspace list, badges, empty states, memory/openflow sections
- Notifications: in-app history, unread tracking, Linux desktop notification integration
- CLI/socket basics: status, app state, workspace creation, pane splitting, terminal write, notify, memory, index
- Project memory: local shared memory store and handoff packet generation
- Indexing: local lexical index, rebuild, status, search

## What Is Only Partial Right Now

- Browser pane: rendered using agent-browser, shares session with CLI commands
- Browser automation: uses agent-browser npm package via npx, controlled via CLI commands
- Browser screenshots: comes from agent-browser session, shared with CLI commands
- Console log capture: command shape exists, but current frontend browser implementation does not populate a real log stream
- Notification sound: config toggle exists, but actual sound playback is not implemented
- OpenFlow runtime: run records, phases, retry, shared communication log handling, and sidebar controls exist; real multi-agent runs now work in some cases, but 15-20 agent stability and dev-server lifecycle handling still need validation

## Major Known Gaps Before Calling It A Linux MVP

- manually validate the core daily-driver workflows end to end on Linux
- finish Phase 15 release-readiness work such as docs, packaging plan, polish, and performance budgets
- decide what parts of OpenFlow are real MVP vs future scaffolding

## Manual Testing Checklist

Use this as the practical checklist for current testing.

### 1. App startup and shell

- [x] App starts on Linux with `npm run tauri dev`
- [x] App still opens with the X11 fallback command when needed
- [ ] Startup failures show a visible error instead of crashing silently / couldnt test, didnt have startup fail to test
- [ ] Theme fallback still works when Omarchy config is missing / havent tested on other device then omarchy

### 2. Workspace management

- [x] Create a new workspace from the sidebar
- [x] Rename a workspace
- [x] Switch between at least two workspaces from the sidebar
- [x] Cycle workspaces with keyboard shortcuts
- [x] Close a workspace and land on a valid fallback workspace / FIXED: closing last workspace now shows empty state with create button

### 3. Terminal workflows

- [x] Initial terminal session starts reliably / right now i have memory so i havent experienced fully fresh start untill i delete local memery of the workspaces
- [x] Split right creates a second independent terminal
- [x] Split down creates a second independent terminal
- [ ] Pane focus changes correctly by click and keyboard / dont know what you mean by this
- [x] Drag resize works in both horizontal and vertical layouts
- [ ] Keyboard resize works on the active pane / dont know what you mean
- [x] Closing one terminal pane keeps the remaining pane tree valid
- [ ] Restart terminal session works after exit/failure / dont know what you mean
- [ ] Terminal output survives frontend remount/re-attach without obvious corruption / dont know what you mean

### 4. Mixed pane layouts

- [x] Swap two terminal panes in the same surface / FIXED: can now drag from header without focusing first
- [x] Create a browser pane next to a terminal pane
- [x] Close a browser pane without damaging the remaining layout
- [x] Layout restores after relaunch with the expected pane tree

### 5. Notifications and attention

- [x] Manual `notify` command appears in the UI / after plressing 'test alert' i see notification badge if thts what you mean
- [x] Unfocused workspaces show unread badges
- [ ] Focusing the workspace clears read state as expected / yes this works but shouldn there be like a glowing radios on the pane showing that this is the pane needing attention and only the badge dispears when hadnling that pane?
- [x] Linux desktop notification appears when requested / works but clicking notification does NOT bring user to workspace. This is a known gap - see details below.

### Notification Click Gap (Requires Deep Dive)

**Current behavior:** Desktop notifications appear with urgency=Critical and Transient=true. The app window is focused when the notification is CREATED (via Tauri + hyprctl), but clicking the notification does NOT focus the app.

**Root cause:** On Wayland with mako notification daemon:
1. Clicking a notification requires the app to define a notification ACTION and listen for D-Bus signals
2. Mako follows FreeDesktop spec - when clicked, it sends "ActionInvoked" via D-Bus
3. Tauri does NOT have built-in D-Bus support for receiving notification click signals
4. VS Code/Cursor work because they use native code with deep D-Bus integration

**What's been tried:**
- Added .desktop file with StartupWMClass
- Added windowClassname to tauri.conf.json  
- Added DesktopEntry hint to notifications
- Added hyprctl focuswindow dispatch on notification creation

**To fix properly (requires significant work):**
- Option A: Add D-Bus service file for codemux to register as notification receiver
- Option B: Use a Tauri plugin for notification handling
- Option C: Implement custom notification action that triggers via mako exec config

This is an architectural limitation, not a quick fix. Defer to Phase 16+ when more resources available.
- [ ] Notification sound toggle persists its setting / dont know what you mean , i havent seend sound toggle

### 6. Browser pane current expectation

- [x] Browser pane can be created from an existing terminal pane
- [x] Browser pane now launches into a live screenshot-driven Chromium surface instead of the old failing iframe path
- [x] Address bar navigation works for resolvable URLs such as `google.com`
- [x] Browser viewport resizes with the pane closely enough to keep major page content visible
- [x] Clicking inside the browser viewport reaches the underlying Chromium page
- [x] Open external browser works reliably
- [x] Browser error/loading state renders when navigation fails
- [ ] Back/forward/reload need a focused manual pass against the new screenshot-driven runtime
- [ ] Text entry inside arbitrary web pages still needs deeper validation beyond address-bar navigation

Current expectation note:

- Do not treat this section as proof of a finished embedded browser. The current pane is a working screenshot-driven Chromium prototype, not a native embedded Tauri webview.
- Expect lower-fidelity interaction than a real embedded browser: periodic screenshot refresh, coordinate-mapped clicks, and CDP-driven navigation/input.

### 7. CLI and socket control

- [x] `cargo run --manifest-path src-tauri/Cargo.toml -- status` works while the app is running / i get a json output when i run this:

```
❯ cargo run --manifest-path src-tauri/Cargo.toml -- status
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.20s
     Running `src-tauri/target/debug/codemux status`
{
  "ok": true,
  "protocol_version": 1,
  "data": {
    "protocol_version": 1,
    "socket_path": "/run/user/1000/codemux.sock"
  },
  "error": null
}
```

- [x] `cargo run --manifest-path src-tauri/Cargo.toml -- json get_app_state` returns valid state

/ about these next socket commands idk how to tun and test

- [ ] Socket command can create a workspace
- [ ] Socket command can split a pane
- [ ] Socket command can send terminal input
- [ ] Socket command can post a notification

### 8. Project memory and indexing

- [ ] Memory can be viewed from CLI
- [ ] Memory can be updated from CLI or UI
- [ ] Handoff packet generation works
- [ ] Index rebuild succeeds on the current repo
- [ ] Index search returns expected matches

### 9. OpenFlow current expectation

- [ ] OpenFlow run can be created from the UI
- [ ] OpenFlow run appears in the sidebar
- [ ] Loop/Next/Pause/Cancel/Retry change run state as expected
- [ ] Timeline and status updates render in the UI

Current expectation note:

- Do not treat this section as proof of a finished autonomous agent system. OpenFlow now runs real agent PTYs and a shared communication log, but the overall workflow is still not release-ready.
- Recent hardening added durable debug logs for wrapper/native launch attribution, a safe `codemux` PATH shim for agent sessions, and blocks bare `codemux` GUI launches from OpenFlow agent sessions. One manual post-fix OpenFlow run completed without spawning duplicate windows, but large-run reliability still needs broader validation.
- The current fix direction is two-layered:
  - use a real cross-platform single-instance mechanism for Codemux itself
  - run OpenFlow agent commands through a generic execution-isolation layer so Linux can sandbox first and macOS/Windows can add backends later

## 10. OpenFlow Workspace (NEW - Phase 1)

### Setup Flow

- [ ] Creating new workspace and selecting "OpenFlow run" skips pane layout options and goes to goal/title input
- [ ] Can enter title and goal for OpenFlow run
- [ ] OpenFlow workspace is created with the entered title

### OpenFlow Workspace UI

- [ ] OpenFlow workspace renders differently from regular workspace
- [ ] Can switch between workspaces when OpenFlow workspace is active
- [ ] Sidebar shows all workspaces (including OpenFlow)
- [ ] Can click on other workspaces in sidebar to switch away from OpenFlow

### Agent Configuration Panel

- [ ] Agent count slider works (2-20)
- [ ] Can select CLI tool per agent (dynamically discovered from PATH at runtime)
- [ ] Can select model per agent (dynamically loaded from `opencode models`, updates when tool changes)
- [ ] Can select thinking mode per agent (auto/none/low/medium/high for opencode)
- [ ] Can select role per agent
- [ ] Goal textarea accepts input
- [ ] Start button creates OpenFlow run

### Orchestration View

- [ ] After starting run, switches to orchestration view
- [ ] Shows agent nodes with roles and status
- [ ] Shows timeline entries
- [ ] Control buttons work: Loop, Next, Pause, Cancel
- [ ] Communication panel shows messages

### Known Issues

- [ ] OpenFlow workspace does not have a browser pane yet (planned for Phase 6)
- [ ] Browser view is still a placeholder toggle rather than a full persistent OpenFlow browser surface
- [ ] 15-20 agent runs still need validation for stray helper launches, Bubblewrap compatibility, and `beforeDevCommand` teardown behavior under `tauri dev`
- [ ] Duplicate app spawn hardening is partially implemented:
  - official Tauri single-instance plugin is now wired into the app as the replacement direction for socket-only singleton logic
  - Linux OpenFlow agent sessions now resolve bare `codemux` calls to the currently running app binary and request the `linux_bubblewrap` execution backend by default
  - the Linux backend keeps network/build/test flows available, hides host GUI/session sockets, and preserves the Codemux control socket for CLI/browser IPC
  - latest manual validation: one OpenFlow run (`openflow-run-54766AF5`) completed without launching duplicate `Codemux` windows or hitting the old `beforeDevCommand` shutdown symptom
  - use `/run/user/$UID/codemux-native-launches.log` to distinguish a real second GUI instance (`component=tauri event=main_window_available`) from a short-lived launch attempt that exits (`outcome=single_instance_exit`); `.codemux/vite-wrapper.log` now includes a `port_probe` line before each Vite start to help diagnose port-1420 conflicts vs normal teardown
- [ ] Startup still logs `Existing control socket ... appears stale; replacing it` on restart, which looks non-fatal in the latest successful run but should be cleaned up so future duplicate-launch investigations are less ambiguous
- Note: WorkspaceType serde was previously serializing as PascalCase ("OpenFlow"); now fixed to snake_case ("open_flow") matching frontend expectations
