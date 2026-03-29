---
title: Agent Status Indicators
description: Real-time visual indicators showing what AI agents are doing across workspaces.
---

# Agent Status Indicators

Codemux tracks the state of AI agents running in your terminals and shows real-time status indicators in the sidebar and tab bar. This lets you monitor multiple agents across workspaces at a glance.

## Status States

| State | Color | Animation | Meaning |
|-------|-------|-----------|---------|
| **Idle** | Hidden | None | Agent is not running or has no pending activity |
| **Working** | Amber | Pulsing dot + braille spinner (sidebar) | Agent is actively processing |
| **Permission** | Red | Pulsing dot | Agent needs user input or approval |
| **Review** | Green | Static dot | Agent finished work, ready for your review |

Permission is the highest priority — if any pane in a workspace needs input, the workspace indicator shows red regardless of what other panes are doing.

## Where Indicators Appear

### Sidebar Workspace Icons

Each workspace in the sidebar shows an aggregate status across all its panes:

- **Working** — The workspace icon is replaced by an animated ASCII spinner (braille pattern)
- **Permission / Review** — A colored dot overlays the top-right of the workspace icon

The highest-priority status across all panes wins.

### Terminal Tab Headers

Each tab shows the highest-priority status from its panes as a small colored dot next to the tab name.

### Terminal Pane Headers

Individual panes show their own status dot in the pane title bar when not idle.

## How It Works

Codemux uses a hook-based system to receive agent lifecycle events:

1. **On startup**, Codemux starts a local HTTP server on a random port (`127.0.0.1:<port>`)
2. **Hook registration** — Codemux writes hook entries to `~/.claude/settings.json` that call a notification script
3. **During agent work**, Claude Code fires hooks at lifecycle events (prompt submitted, tool used, permission needed, work complete)
4. **The hook script** (`~/.codemux/hooks/notify.sh`) sends an HTTP request to Codemux's hook server with the event type and session ID
5. **Codemux maps** the event to a pane status and updates the UI in real-time

### Event Mapping

| Claude Code Event | Status |
|-------------------|--------|
| `UserPromptSubmit`, `PostToolUse`, `BeforeAgent` | Working |
| `PermissionRequest`, `Notification` | Permission |
| `Stop`, `AfterAgent` | Review (or Idle if you're looking at the pane) |

### Smart Idle Detection

When an agent finishes (Stop event), Codemux checks whether you're currently viewing that pane:

- **Active pane in active workspace** — Status clears to Idle (you're already looking at it)
- **Background pane or workspace** — Status stays as Review until you click on that tab

## Supported Agents

- **Claude Code** — Full hook integration with real-time status tracking
- Other agents (Codex, OpenCode, Gemini) do not currently support hooks, so they won't show status indicators

## Hook Safety

The hooks Codemux writes to `~/.claude/settings.json` are safe:

- The notification script checks for `CODEMUX_HOOK_PORT` and `CODEMUX_SESSION_ID` environment variables before doing anything
- If Codemux isn't running, the hook silently exits (no errors, no side effects)
- Hook requests use a 1-second connect timeout and 2-second max time
- Codemux preserves any existing hooks from other tools — it only adds/updates its own entries

## Troubleshooting

If status indicators aren't working:

1. Check that `~/.claude/settings.json` contains Codemux hook entries (look for paths containing `.codemux/hooks/notify.sh`)
2. Check that `~/.codemux/hooks/notify.sh` exists and is executable
3. Restart Codemux — hooks are registered on startup
