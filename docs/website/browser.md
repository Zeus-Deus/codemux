---
title: Browser
description: Embedded browser pane with screenshot-based rendering, agent automation, and viewport sync.
---

# Browser

Codemux includes an embedded browser pane for viewing web apps, running tests, and letting AI agents interact with pages programmatically.

## How It Works

The browser uses screenshot-based rendering with a 1-second refresh cycle. A headless browser runs in the background, captures screenshots, and streams them to the pane via WebSocket. User clicks are mapped from display coordinates to the actual viewport.

## Browser Toolbar

The toolbar at the top of every browser pane includes:

- **URL bar** — Type a URL and press Enter to navigate. Bare domains auto-prefix with `https://`.
- **Back / Forward** — Standard navigation history
- **Refresh** — Reload the current page
- **Home** — Reset to `about:blank`
- **External link** — Open the current URL in your system browser

## Creating a Browser Pane

- Press `Ctrl+Shift+B` to create a new browser tab
- Or click the `+` dropdown in the tab bar and select "Browser"
- Or use the Shell + Browser workspace preset

## Agent Automation

AI agents running in Codemux terminals can control the browser programmatically. See [Browser Agent Commands](browser-agent-commands.md) for the full reference.

```bash
codemux browser open http://localhost:3000
codemux browser snapshot          # Get accessibility tree
codemux browser click "#submit"   # Click an element
codemux browser fill "#email" "test@example.com"
codemux browser screenshot        # Capture as base64 PNG
codemux browser console-logs      # Get console output
```

## Viewport Sync

When the browser pane resizes, the viewport dimensions sync automatically. The rendered page adapts to the pane size.

## Limitations

- Screenshot-driven rendering (not a native embedded webview)
- Lower interaction fidelity than a native browser
- Console logs are captured but not a full live stream
- Click accuracy depends on screenshot-to-viewport coordinate mapping
