# Browser Agent Commands

- Purpose: Complete reference of browser commands available to AI agents running in Codemux terminals.
- Audience: AI agents (Claude, GPT, etc.) that need to control the browser pane programmatically.
- Authority: Canonical browser command reference. Source of truth for CLI syntax and available actions.
- Update when: Browser commands are added, removed, or their syntax changes.
- Read next: `AGENTS.md`, `docs/reference/CONTROL.md`

## Detect Codemux

Before using browser commands, confirm you are inside Codemux:

```bash
if [ -n "$CODEMUX_WORKSPACE_ID" ]; then
  # Inside Codemux — browser commands available
fi
```

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

Returns the page's accessibility tree. Use this to discover what elements exist before clicking or filling. The `browser_id` is optional (defaults to the active browser).

### Click an Element

```bash
codemux browser click <selector> [browser_id]
```

Clicks the element matching the CSS selector. Examples:

```bash
codemux browser click "#submit-button"
codemux browser click "button[type='submit']"
codemux browser click ".nav-link:first-child"
```

### Fill an Input

```bash
codemux browser fill <selector> <value> [browser_id]
```

Sets the value of an input field. Clears existing content first.

```bash
codemux browser fill "#email" "user@example.com"
codemux browser fill "input[name='search']" "query text"
```

### Take a Screenshot

```bash
codemux browser screenshot [browser_id]
```

Returns a base64-encoded PNG of the current page.

### Get Console Logs

```bash
codemux browser console-logs [browser_id]
```

Returns JavaScript console output from the page. Useful for debugging errors.

## Socket API Commands

These actions are available via the Codemux control socket. They cover additional functionality not exposed as CLI subcommands.

Socket path: `/run/user/$UID/codemux.sock`

### General Format

```bash
echo '{"command":"browser_automation","params":{"browser_id":"default","action":{"kind":"<action>", ...}}}' | nc -U /run/user/$(id -u)/codemux.sock
```

### Available Actions

#### open / open_url

Navigate to a URL.

```json
{"command":"browser_automation","params":{"browser_id":"default","action":{"kind":"open","url":"https://example.com"}}}
```

#### screenshot

Take a screenshot (base64 PNG).

```json
{"command":"browser_automation","params":{"browser_id":"default","action":{"kind":"screenshot"}}}
```

#### snapshot / accessibility_snapshot

Get the accessibility tree.

```json
{"command":"browser_automation","params":{"browser_id":"default","action":{"kind":"snapshot"}}}
```

#### click

Click an element by CSS selector.

```json
{"command":"browser_automation","params":{"browser_id":"default","action":{"kind":"click","selector":"#submit"}}}
```

#### fill

Fill an input field.

```json
{"command":"browser_automation","params":{"browser_id":"default","action":{"kind":"fill","selector":"#email","text":"user@example.com"}}}
```

#### type_text

Type text on the page body (simulates keypresses).

```json
{"command":"browser_automation","params":{"browser_id":"default","action":{"kind":"type_text","text":"hello world"}}}
```

#### evaluate / eval

Run JavaScript on the page and return the result.

```json
{"command":"browser_automation","params":{"browser_id":"default","action":{"kind":"evaluate","script":"document.title"}}}
```

#### back

Navigate back in history.

```json
{"command":"browser_automation","params":{"browser_id":"default","action":{"kind":"back"}}}
```

#### forward

Navigate forward in history.

```json
{"command":"browser_automation","params":{"browser_id":"default","action":{"kind":"forward"}}}
```

#### reload

Reload the current page.

```json
{"command":"browser_automation","params":{"browser_id":"default","action":{"kind":"reload"}}}
```

#### viewport

Set the browser viewport size.

```json
{"command":"browser_automation","params":{"browser_id":"default","action":{"kind":"viewport","width":1024,"height":768}}}
```

#### console / console_logs

Get console output.

```json
{"command":"browser_automation","params":{"browser_id":"default","action":{"kind":"console"}}}
```

### Create a Browser Pane (Socket)

```json
{"command":"create_browser_pane","params":{"pane_id":""}}
```

### Open a URL (Socket Shorthand)

```json
{"command":"open_url","params":{"browser_id":"default","url":"https://example.com"}}
```

## Tips

1. Always run `codemux browser snapshot` before interacting — know what elements exist.
2. Use explicit CSS selectors, not guesses.
3. Check `codemux browser console-logs` when something fails silently.
4. The `browser_id` parameter is optional in all CLI commands. It defaults to the active browser pane.
5. Never use `xdg-open`, `open`, or any command that opens the system browser.
6. Test incrementally: navigate, snapshot, interact, snapshot again.
