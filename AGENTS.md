# Codemux Agent Guide

This file has two jobs:

- bootstrap agents working on the Codemux repository itself
- explain how agents should use Codemux browser automation

## Repo Development Bootstrap

Use this section when the task is about developing the Codemux repository itself.

- start with `WORKFLOW.md` and `docs/INDEX.md`
- read the relevant canonical docs before assuming project state
- treat `docs/` as the maintained project docs system
- if the docs feel stale or messy, use `docs/reference/DOCS_REINDEX.md`

These instructions are project-scoped. They matter when an agent is working inside this repository, not when Codemux is later used on unrelated projects.

## Codemux Browser Automation

Use this section when an agent needs to control the browser inside Codemux.

For project-wide context, start with `WORKFLOW.md` and `docs/INDEX.md`. This file stays focused on agent operating behavior inside Codemux itself.

## Always Use Codemux Browser Commands

**NEVER use these commands:**

- `xdg-open` - opens in system browser, not Codemux
- `open` (macOS) - opens in system browser
- any other command that opens the default system browser

**ALWAYS use these instead:**

- `codemux browser open <url>` - opens a URL in Codemux's browser pane

The browser automation runs against the browser pane inside Codemux, not your system browser.

## Prerequisites

Before using browser commands, ensure a browser pane exists in your Codemux workspace:

1. Create a browser pane.
2. Keep the pane visible or active so the commands have a target.

## Quick Start

When working inside Codemux, you have access to browser automation through CLI commands:

```bash
codemux browser create
codemux browser open https://example.com
codemux browser snapshot
codemux browser click "#submit-button"
codemux browser fill "#email" "test@example.com"
codemux browser screenshot
codemux browser console-logs
```

## Environment Variables

Codemux sets these environment variables in terminals running inside it:

- `CODEMUX_WORKSPACE_ID` - current workspace ID
- `CODEMUX_SURFACE_ID` - current terminal surface ID

You can use these to detect if you are running inside Codemux:

```bash
if [ -n "$CODEMUX_WORKSPACE_ID" ]; then
    echo "We are inside Codemux"
fi
```

## Common Workflows

### Testing a web app

```bash
npm run dev
codemux browser open http://localhost:3000
codemux browser snapshot
codemux browser fill "#search" "test query"
codemux browser click "#submit"
codemux browser snapshot
```

### Building and testing a form

```bash
codemux browser open http://localhost:5173/form
codemux browser fill "#name" "John Doe"
codemux browser fill "#email" "john@example.com"
codemux browser fill "#password" "secret123"
codemux browser click "button[type='submit']"
codemux browser snapshot
codemux browser screenshot
```

### Debugging JavaScript errors

```bash
codemux browser console-logs
codemux browser snapshot
```

## Available Commands

| Command | Description |
| --- | --- |
| `codemux browser create` | Create a new browser pane in the current workspace |
| `codemux browser open <url>` | Navigate to a URL |
| `codemux browser snapshot` | Get the accessibility tree |
| `codemux browser click <selector>` | Click an element |
| `codemux browser fill <selector> <text>` | Fill an input |
| `codemux browser screenshot` | Take a screenshot |
| `codemux browser console-logs` | Get console logs |

Additional actions available via the socket API: `back`, `forward`, `reload`, `evaluate` (run JS), `type_text`, `viewport`. See `docs/reference/BROWSER-AGENT-COMMANDS.md` for the full reference.

## Socket API

You can also control Codemux via JSON commands over the socket:

```bash
echo '{"command":"browser_automation","params":{"browser_id":"default","action":{"kind":"open_url","url":"https://example.com"}}}' | nc -U /run/user/1000/codemux.sock
```

## Tips For Agents

1. Check whether you are inside Codemux with `CODEMUX_WORKSPACE_ID`.
2. Get a snapshot before interacting so you know what elements exist.
3. Prefer explicit selectors or refs instead of guessing.
4. Check console logs when browser behavior is unclear.
5. Test incrementally.
6. Always use explicit `codemux browser ...` subcommands and never invoke bare `codemux` from an agent terminal.

## Browser Vs Terminal

- terminal pane: run your dev server, build commands, and tests
- browser pane: view and interact with the running app inside Codemux
