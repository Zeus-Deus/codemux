# Codemux Agent Guide

This guide explains how AI coding agents can use Codemux's browser automation when building web applications.

## IMPORTANT: Always Use Codemux Browser Commands

**NEVER use these commands:**

- `xdg-open` - opens in system browser, not Codemux
- `open` (macOS) - opens in system browser
- Any other command that opens the default system browser

**ALWAYS use these instead:**

- `codemux browser open <url>` - opens in Codemux's embedded browser pane

The browser automation runs against the **embedded browser pane inside Codemux**, NOT your system browser.

## Prerequisites

Before using browser commands, ensure a browser pane exists in your Codemux workspace:

1. Create a browser pane (via sidebar or split menu)
2. The pane must be visible/active for commands to work

## Quick Start

When working inside Codemux, you have access to browser automation through CLI commands:

```bash
# Create a browser pane (required first step - without this, browser commands won't work)
codemux browser create

# Open a URL in the browser
codemux browser open https://example.com

# Get the page's accessibility tree (useful for finding elements)
codemux browser snapshot

# Click an element (by CSS selector)
codemux browser click "#submit-button"

# Fill a form input
codemux browser fill "#email" "test@example.com"

# Take a screenshot
codemux browser screenshot

# Get console logs
codemux browser console-logs
```

## Environment Variables

Codemux sets these environment variables in terminals running inside it:

- `CODEMUX_WORKSPACE_ID` - Current workspace ID
- `CODEMUX_SURFACE_ID` - Current terminal surface ID

You can use these to detect if you're running inside Codemux:

```bash
if [ -n "$CODEMUX_WORKSPACE_ID" ]; then
    echo "We're inside Codemux!"
fi
```

## Common Workflows

### Testing a web app

```bash
# 1. Start your dev server in one terminal pane
npm run dev

# 2. Open the app in browser
codemux browser open http://localhost:3000

# 3. Get the page structure
codemux browser snapshot
# Output: - textbox "Search" [ref=e1]
#         - button "Submit" [ref=e2]

# 4. Interact with the app
codemux browser fill "#search" "test query"
codemux browser click "#submit"

# 5. Verify the result
codemux browser snapshot
```

### Building and testing a form

```bash
# Open the form
codemux browser open http://localhost:5173/form

# Fill out the form
codemux browser fill "#name" "John Doe"
codemux browser fill "#email" "john@example.com"
codemux browser fill "#password" "secret123"

# Submit
codemux browser click "button[type='submit']"

# Check for success message
codemux browser snapshot

# Take a screenshot for evidence
codemux browser screenshot
```

### Debugging JavaScript errors

```bash
# Get console logs
codemux browser console-logs

# Get page errors
codemux browser console-logs

# Re-check page structure after an error
codemux browser snapshot
```

## Available Commands

| Command                                  | Description                                        |
| ---------------------------------------- | -------------------------------------------------- |
| `codemux browser create`                 | Create a new browser pane in the current workspace |
| `codemux browser open <url>`             | Navigate to URL                                    |
| `codemux browser snapshot`               | Get accessibility tree                             |
| `codemux browser click <selector>`       | Click element                                      |
| `codemux browser fill <selector> <text>` | Fill input                                         |
| `codemux browser screenshot`             | Take screenshot                                    |
| `codemux browser console-logs`           | Get console logs                                   |

## Socket API

You can also control Codemux via JSON commands over the socket:

```bash
# Send JSON command
echo '{"command":"browser_automation","params":{"browser_id":"default","action":{"kind":"open_url","url":"https://example.com"}}}' | nc -U /run/user/1000/codemux.sock
```

## Tips for Agents

1. **Always check if running inside Codemux** - Use `CODEMUX_WORKSPACE_ID` env var to detect
2. **Always get a snapshot first** - This tells you what elements are available
3. **Use CSS selectors** - They're most reliable for targeting elements
4. **Check console logs** - When something doesn't work, look for JS errors
5. **Take screenshots** - Useful for debugging and showing results to users
6. **Test incrementally** - Make one change, test it, then move on
7. **Always use explicit `codemux browser ...` subcommands** - Never invoke bare `codemux` from an agent terminal

## Browser vs Terminal

- **Terminal pane**: Run your dev server, build commands, tests
- **Browser pane**: View and interact with your running app

The browser automation runs against the browser pane in Codemux, allowing you to test your web apps without leaving the terminal.
