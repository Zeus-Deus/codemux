---
title: CLI
description: Control a running Codemux instance from your terminal with the codemux command.
---

# CLI

The `codemux` command lets you control a running Codemux instance from any terminal. Agents use it to interact with the browser, manage project memory, and send notifications programmatically.

## How It Connects

The CLI communicates with Codemux over a Unix socket. The socket is created at:

- `$XDG_RUNTIME_DIR/codemux.sock` (standard path)
- `/tmp/codemux-{uid}/codemux.sock` (fallback when `XDG_RUNTIME_DIR` is unset)

All commands send a JSON request to the socket and receive a JSON response. If Codemux isn't running, commands fail with a connection error.

## Commands

### Browser Control

Control the embedded browser pane from the terminal. This is the primary way AI agents interact with web pages.

```bash
codemux browser open <url>              # Navigate to a URL
codemux browser snapshot                # Get accessibility tree
codemux browser snapshot --dom          # Get DOM-based element tree
codemux browser click "<selector>"      # Click element by CSS selector
codemux browser fill "<selector>" "text" # Type into an input field
codemux browser screenshot              # Capture viewport as base64 PNG
codemux browser console-logs            # Get browser console output
```

Coordinate-based commands for iframes, shadow DOM, canvas, and protected inputs:

```bash
codemux browser click-at <x> <y>                     # Click at pixel coordinates
codemux browser type-at "text" --x <x> --y <y>       # Type at coordinates
codemux browser scroll-at <x> <y> --direction down    # Scroll at coordinates
codemux browser key-press "Enter"                     # Send keyboard event
codemux browser drag <x1> <y1> <x2> <y2>             # Drag between points
```

OS-level input via ydotool (stealth mode, requires headed browser + Hyprland):

```bash
codemux browser click-os <x> <y>                     # OS-level click
codemux browser type-os "text" --x <x> --y <y>       # OS-level typing
```

See [Browser Agent Commands](browser-agent-commands.md) for full details.

### Project Memory

Store and retrieve project context that persists across agent sessions.

```bash
codemux memory show                                         # Display project memory
codemux memory set --goal "Ship auth feature" --focus "OAuth flow"  # Update memory fields
codemux memory add decision "Use JWT for sessions" --tag auth       # Add a memory entry
```

Memory entry kinds: `pinned_context`, `decision`, `next_step`, `session_summary`.

### Code Index

Build and search a code index for the current project.

```bash
codemux index build                    # Build/rebuild the search index
codemux index status                   # Show index status
codemux index search "handleAuth" --limit 20  # Search indexed code
```

### Other Commands

```bash
codemux status                         # Check if Codemux is running
codemux notify "Build complete"        # Send a notification
codemux handoff                        # Generate a project handoff summary
codemux capabilities                   # List all commands as JSON
```

## How Agents Use It

Every terminal session in Codemux has environment variables that tell agents they're running inside Codemux:

| Variable | Description |
|----------|-------------|
| `CODEMUX` | Set to `1` when running inside Codemux |
| `CODEMUX_VERSION` | Codemux version |
| `CODEMUX_WORKSPACE_ID` | Current workspace ID |
| `CODEMUX_SESSION_ID` | Terminal session ID |
| `CODEMUX_BROWSER_CMD` | Browser command prefix (`codemux browser`) |
| `CODEMUX_AGENT_CONTEXT` | System prompt telling agents to use Codemux's browser |

Claude Code receives `CODEMUX_AGENT_CONTEXT` via `--system-prompt`, which instructs it to use `codemux browser` commands instead of launching system browsers or headless Chromium.

## MCP Server

Codemux also exposes its control interface as an MCP server (`codemux mcp`), providing the same browser, workspace, and git tools over JSON-RPC for AI agents that support the Model Context Protocol.
