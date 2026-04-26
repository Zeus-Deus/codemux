# Notifications

- Purpose: Describe the current capability and constraints of the notification system.
- Audience: Anyone working on notifications, alerts, or attention workflows.
- Authority: Canonical feature-level reality doc.
- Update when: Notification delivery, display, or desktop integration changes.
- Read next: `docs/reference/FEATURES.md`

## What This Feature Is

Codemux has a multi-layer notification system: workspace-scoped alert badges in the sidebar, desktop notifications via the system notification daemon, and global toast notices for errors and status messages.

## Current Model

Notifications originate from the Rust backend. Desktop notifications use `notify_rust::Notification` with critical urgency and the `com.codemux.app` desktop entry. Workspace alerts are stored in app state and surfaced in the sidebar with unread badge counts. The frontend subscribes to state change events to update notification UI.

## What Works Today

- Workspace-scoped alert notifications with severity levels (info, attention)
- Sidebar notification section with unread badge counts per workspace
- Expandable alert list with message preview (2-line clamp) and timestamps
- Mark all read button
- Desktop notifications via system notification daemon (notify-rust / libnotify)
- Agent-completion desktop notifications fire from the hook server when an agent's pane is not currently visible
- Notification sound toggle in sidebar footer and settings (Linux uses `paplay` with the freedesktop `complete.oga`; macOS uses `afplay` with Glass.aiff; Windows uses PowerShell SystemSounds)
- Click-to-focus on Linux: notify-rust `wait_for_action` listens for the libnotify default action and focuses the Codemux window, including a `hyprctl dispatch focuswindow` to jump back to whichever Hyprland workspace the app lives on
- Global toast notices for errors and status messages (bottom-right)
- Agent status indicators (red/amber/green dots) in sidebar and tab bar for Claude Code sessions

## Current Constraints

- Sound playback uses system sounds; no in-app ringtone selection or per-volume control yet
- No notification filtering or per-type muting
- No notification history beyond current session
- macOS / Windows click-to-focus relies on platform notification handling rather than an explicit action listener

## Important Touch Points

- `src-tauri/src/notifications.rs` — desktop notification dispatch, sound playback, click-to-focus action handler
- `src-tauri/src/hooks.rs` — `handle_lifecycle_event` fires the agent-completion notification when the pane is not in the active workspace (or the window is unfocused)
- `src-tauri/src/commands/workspace.rs` — `notify_attention()` (MCP-driven attention requests), `set_notification_sound_enabled()`
- `src/components/ui/status-indicator.tsx` — agent status dots (permission/working/review)
- `src/components/layout/app-sidebar.tsx` — sidebar notification badges
- `src/tauri/events.ts` — event subscriptions for state changes
