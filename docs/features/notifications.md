# Notifications

- Purpose: Describe the current capability and constraints of the notification system.
- Audience: Anyone working on notifications, alerts, or attention workflows.
- Authority: Canonical feature-level reality doc.
- Update when: Notification delivery, display, or desktop integration changes.
- Read next: `docs/reference/FEATURES.md`

## What This Feature Is

Codemux has a multi-layer notification system: workspace-scoped alert badges in the sidebar, desktop notifications via the system notification daemon, and global toast notices for errors and status messages.

## Current Model

Notifications originate from the Rust backend. Desktop notifications use `notify_rust::Notification` with normal urgency and the `com.codemux.app` desktop entry. Normal urgency lets every common Linux notification daemon (mako, dunst, GNOME Shell, KDE Plasma, xfce4-notifyd) auto-dismiss popups after their configured timeout — Critical is reserved by those daemons for non-expiring system emergencies and would leave Codemux popups on screen until the user clicked them. Workspace alerts are stored in app state and surfaced in the sidebar with unread badge counts. The frontend subscribes to state change events to update notification UI.

## What Works Today

- Workspace-scoped alert notifications with severity levels (info, attention)
- Sidebar notification section with unread badge counts per workspace
- Expandable alert list with message preview (2-line clamp) and timestamps
- Mark all read button
- Desktop notifications via system notification daemon (notify-rust / libnotify)
- Agent-completion desktop notifications fire from the hook server when an agent's pane is not currently visible
- Notification sound toggle in **Settings only** (`settings-view.tsx` is the sole caller of `setNotificationSoundEnabled`; the sidebar footer has no bell/volume control) (Linux uses `paplay` with the freedesktop `complete.oga`; macOS uses `afplay` with Glass.aiff; Windows uses PowerShell SystemSounds)
- Click-to-focus on Linux: notify-rust `wait_for_action` listens for the libnotify default action and focuses the Codemux window, including a `hyprctl dispatch focuswindow` to jump back to whichever Hyprland workspace the app lives on
- Global toast notices for errors and status messages (bottom-right)
- Agent status indicators (red/amber/green dots) in sidebar and tab bar for **Claude Code, Codex, Gemini, and OpenCode** sessions (the hook server tracks per-adapter status; Claude Code's 60s idle reminder is dropped by the hook script so it no longer raises a phantom red dot). **Pi** has no distinct permission/approval event, so it only gets the amber/green working cadence — never the red needs-input dot
- **Per-worktree mute**: a right-click → "Mute notifications" action on the sidebar workspace row toggles `notifications_muted` via the `set_workspace_muted` Tauri command. Muted workspaces show a sidebar bell-off icon and skip agent-completion notifications without changing global sound state.

## Current Constraints

- Sound playback uses system sounds; no in-app ringtone selection or per-volume control yet
- No notification history beyond current session
- macOS / Windows click-to-focus relies on platform notification handling rather than an explicit action listener

## The alert list no longer has a frontend

`AppStateSnapshot.notifications` (`NotificationSnapshot[]`, produced by
`add_notification` in `state_impl.rs`) is **never read by any frontend file**.
There is no expandable alert list, no message preview, no timestamps view, and
no "Mark all read" button — nor any mark-read Tauri command to back one. The
only surviving in-app surface is the numeric `notification_count` badge on the
sidebar inbox card. Desktop/OS notifications themselves are unaffected.

Per-worktree mute **is** still reachable: "Mute notifications" lives in
`WorkspaceContextMenuItems`, which the live inbox card's right-click menu
renders via `workspace-inbox-menu.tsx`.

## Important Touch Points

- `src-tauri/src/notifications.rs` — desktop notification dispatch, sound playback, click-to-focus action handler
- `src-tauri/src/hooks.rs` — `handle_lifecycle_event` fires the agent-completion notification when the pane is not in the active workspace (or the window is unfocused); gates **only** `dispatch_agent_complete` on mute — `set_pane_status_by_session` runs unconditionally, so a muted workspace still updates its status dot. Separately, Claude Code's 60s idle reminder is dropped by the hook *script* inspecting the payload message before it reaches the server, not by `hooks.rs`
- `src-tauri/src/commands/workspace.rs` — `notify_attention()` (MCP-driven attention requests), `set_notification_sound_enabled()`, `set_workspace_muted()`
- `src/components/ui/status-indicator.tsx` — agent status dots in the tab bar, title-bar tabs, and pane headers (the sidebar inbox card renders its own `text-status-working` / `status-attention` tokens instead) (permission/working/review)
- `src/components/layout/sidebar-inbox-card.tsx` — the per-workspace `notification_count` badge
- `src/components/layout/sidebar-workspace-row.tsx` — per-worktree mute toggle + bell-off icon
- `src/tauri/events.ts` — event subscriptions for state changes
