---
title: Browser
description: Embedded browser pane with screenshot-based rendering, agent automation, and viewport sync.
---

# Browser

Codemux includes an embedded browser pane for viewing web apps, running tests, and letting AI agents interact with pages programmatically.

## How It Works

The browser uses screenshot-based rendering with a 1-second refresh cycle. A Chromium instance runs in the background, captures screenshots, and streams them to the pane via WebSocket. User clicks are mapped from display coordinates to the actual viewport.

### Stealth Mode

The browser launches with anti-detection flags to avoid triggering bot protection on sites like Cloudflare:

- Removes `navigator.webdriver` detection
- Spoofs user-agent to match the installed Chrome version
- Disables automation-related Blink features and infobars
- Disables background throttling for consistent behavior

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

## Hybrid Input System

Browser automation uses a three-tier input architecture. Agents choose the appropriate tier based on the target:

### Tier 1: Selector-based (fastest)

Standard CSS selector interaction via CDP. Best for regular web apps.

- `browser_click` — Click element by CSS selector
- `browser_fill` — Fill input by CSS selector

### Tier 2: Coordinate-based CDP (vision-capable)

Pixel-coordinate interaction via Chrome DevTools Protocol. Works on iframes, shadow DOM, canvas, and protected form fields.

- `browser_click_at` — Click at (x, y) coordinates
- `browser_type_at` — Click at coordinates then type text
- `browser_scroll_at` — Scroll at coordinates
- `browser_key_press` — Send keyboard events
- `browser_drag` — Drag from one point to another

Mouse movements use Bezier curve interpolation with randomized control points for human-like motion.

### Tier 3: OS-level input (stealth)

Kernel-level input events via `ydotool` that are indistinguishable from real human interaction. Bypasses anti-bot detection (Cloudflare Turnstile, etc.).

- `browser_click_os` — OS-level click at coordinates
- `browser_type_os` — OS-level keyboard typing

Requires `ydotool` + `ydotoold` daemon, headed browser mode, and Hyprland window manager for window geometry.

## Limitations

- Screenshot-driven rendering (not a native embedded webview)
- Console logs are captured but not a full live stream
- OS-level input (Tier 3) requires ydotool and headed mode
- Tier 3 input requires Hyprland for window geometry detection
