---
title: Notifications
description: Desktop notifications, attention badges, and status indicators for monitoring agents across workspaces.
---

# Notifications

Codemux has a notification and attention system that keeps you aware of what's happening across workspaces — especially useful when running multiple agents in parallel.

## Desktop Notifications

When an agent needs attention, Codemux sends a desktop notification via D-Bus (using `notify-rust`). The notification:

- Shows "Codemux" as the title with the event message as the body
- Uses critical urgency so it's not silently dismissed
- Is transient — it disappears after the system timeout

Desktop notifications can be toggled in [Settings](settings.md) > Notifications.

## Window Focus

When a notification fires, Codemux automatically brings its window to the foreground:

- Unminimizes and focuses the window via Tauri
- On Hyprland, also runs `hyprctl dispatch focuswindow class:com.codemux.app` for reliable window activation

This means you don't have to manually find the Codemux window when an agent finishes or needs input.

## Attention Badges

Each workspace in the sidebar shows a yellow notification count badge when it has unread notifications. The count resets when you switch to that workspace.

This lets you scan the sidebar to see which workspaces have pending activity without clicking through each one.

## Pane and Tab Status Indicators

Individual panes and tabs show colored status dots based on agent activity:

| State | Color | Animation | Meaning |
|-------|-------|-----------|---------|
| Idle | Hidden | None | No agent activity |
| Working | Amber | Pulsing | Agent is processing |
| Permission | Red | Pulsing | Agent needs your input |
| Review | Green | Static | Agent finished, ready for review |

These appear in three places:

- **Pane headers** — Each pane shows its own status
- **Tab bar** — Each tab shows the highest-priority status from its panes
- **Sidebar** — Each workspace shows an aggregate status (permission > working > review > idle)

See [Agent Status Indicators](agent-status.md) for the full details on how status tracking works.

## Sending Notifications

Agents can send notifications programmatically:

```bash
codemux notify "Build finished successfully"
```

The MCP server also exposes a `notify` tool with an optional `level` parameter (`info` or `attention`).

## Settings

Two notification settings are available in [Settings](settings.md) > Notifications:

- **Desktop notifications** — Toggle system notifications on/off
- **Notification sounds** — Toggle for notification sounds (the setting exists but sound playback is not yet implemented)

These settings sync across devices via your account.
