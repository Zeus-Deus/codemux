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
- Desktop notification triggers window focus, raise, and Hyprland window manager integration
- Notification sound toggle in sidebar footer and settings
- Global toast notices for errors and status messages (bottom-right)
- Agent status indicators (red/amber/green dots) in sidebar and tab bar for Claude Code sessions

## Current Constraints

- Notification sound toggle exists in state, but actual audio playback is not implemented
- Notification click-to-focus on Wayland and mako still needs deeper D-Bus or native handling
- No notification filtering or per-type muting
- No notification history beyond current session

## Important Touch Points

- `src-tauri/src/commands/workspace.rs` — `notify_attention()`, `set_notification_sound_enabled()`
- `src/components/ui/status-indicator.tsx` — agent status dots (permission/working/review)
- `src/components/layout/app-sidebar.tsx` — sidebar notification badges
- `src/tauri/events.ts` — event subscriptions for state changes
