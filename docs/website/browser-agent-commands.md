---
title: Browser Agent Commands
description: CLI and socket API reference for controlling the embedded browser from AI agents.
---

# Browser Agent Commands

AI agents running in Codemux terminals can control the embedded browser programmatically using CLI commands or the socket API.

## Detect Codemux

Check if you're inside Codemux before using browser commands:

```bash
if [ -n "$CODEMUX_WORKSPACE_ID" ]; then
  # Inside Codemux — browser commands available
fi
```

Environment variables set by Codemux:

- `CODEMUX_WORKSPACE_ID` — Current workspace ID
- `CODEMUX_SURFACE_ID` — Current terminal surface ID

## Setup

Create a browser pane first (only needed once per workspace):

```bash
codemux browser create
```

## CLI Commands

### Navigate

```bash
codemux browser open <url>
```

Opens a URL in the browser pane. Always use this instead of `xdg-open` or `open`.

### Get Accessibility Snapshot

```bash
codemux browser snapshot [browser_id]
```

Returns the page's accessibility tree. Use this to discover elements before interacting.

### Click

```bash
codemux browser click <selector> [browser_id]
```

Clicks an element matching the CSS selector.

### Fill Input

```bash
codemux browser fill <selector> <text> [browser_id]
```

Fills an input field with text.

### Screenshot

```bash
codemux browser screenshot [browser_id]
```

Takes a screenshot and returns it as base64-encoded PNG.

### Console Logs

```bash
codemux browser console-logs [browser_id]
```

Returns captured console output from the page.

## Socket API

For programmatic control, send JSON commands over the Unix socket at `$XDG_RUNTIME_DIR/codemux.sock`:

```bash
echo '{"command":"browser_automation","params":{"browser_id":"default","action":{"kind":"open_url","url":"https://example.com"}}}' | nc -U /run/user/1000/codemux.sock
```

### Available Actions

| Action | Description |
|--------|-------------|
| `open_url` | Navigate to a URL |
| `screenshot` | Capture screenshot |
| `snapshot` | Get accessibility tree |
| `click` | Click an element by selector |
| `fill` | Fill an input field |
| `type_text` | Type text (character by character) |
| `evaluate` | Run JavaScript in the page |
| `back` | Go back in history |
| `forward` | Go forward in history |
| `reload` | Reload the page |
| `viewport` | Set viewport dimensions |
| `console` | Get console logs |

## Common Workflows

### Testing a Web App

```bash
npm run dev &
codemux browser open http://localhost:3000
codemux browser snapshot
codemux browser fill "#search" "test query"
codemux browser click "#submit"
codemux browser snapshot
```

### Debugging JavaScript Errors

```bash
codemux browser console-logs
codemux browser snapshot
```

## Tips

1. Always get a snapshot before interacting — know what elements exist
2. Prefer explicit CSS selectors over guessing
3. Check console logs when behavior is unexpected
4. The `browser_id` parameter is optional — defaults to the active browser
