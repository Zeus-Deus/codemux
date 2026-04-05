# Codemux Agent Environment

You are running inside Codemux, an Agentic Development Environment (ADE).

**Detect:** `test -n "$CODEMUX"`

## Repo Development

When working on the Codemux repository itself:
- Start with `WORKFLOW.md` and `docs/INDEX.md`
- Read relevant canonical docs before assuming project state
- Treat `docs/` as the maintained project docs system
- If docs feel stale, use `docs/reference/DOCS_REINDEX.md`

## Browser Control

**Never** use `xdg-open`, `open`, or system browsers. Use these instead:

| Command | Description |
|---------|-------------|
| `codemux browser open <url>` | Navigate the browser pane to a URL |
| `codemux browser snapshot --dom` | List all interactive elements with CSS selectors |
| `codemux browser snapshot` | Get the accessibility tree |
| `codemux browser click "<selector>"` | Click an element by CSS selector |
| `codemux browser fill "<selector>" "<text>"` | Type into an input field |
| `codemux browser screenshot` | Capture screenshot (base64 PNG) |
| `codemux browser console-logs` | Get browser console output |
| `codemux browser create` | Create a new browser pane |

The user sees the browser pane live while you control it.

**Workflow: Always snapshot before interacting.**
```bash
codemux browser open http://localhost:3000
codemux browser snapshot --dom    # See what's on the page
codemux browser click "#submit"   # Interact with a known element
```

## Git Integration

- Standard git commands work normally
- Changes appear live in the Codemux sidebar Changes panel
- Use conventional commit format (feat:, fix:, docs:, etc.)
- AI commit message generator available in the UI

## Notifications

When you finish a task or need user attention, the user gets notified via Codemux's notification system.

## Memory & Index

| Command | Description |
|---------|-------------|
| `codemux memory show` | Show project memory |
| `codemux memory set --goal "..."` | Set current goal |
| `codemux memory add decision "..."` | Record a decision |
| `codemux index build` | Build code search index |
| `codemux index search "<query>"` | Search indexed code |

## Environment Variables

Set automatically in all Codemux terminals:

| Variable | Value | Purpose |
|----------|-------|---------|
| `CODEMUX` | `1` | Detect Codemux environment |
| `CODEMUX_VERSION` | `0.1.0` | Codemux version |
| `CODEMUX_WORKSPACE_ID` | workspace ID | Current workspace |
| `CODEMUX_BROWSER_CMD` | `codemux browser` | Browser command prefix |
| `BROWSER` | `codemux browser open` | Standard URL handler override |

## Agent Launch Integration

### Claude Code Launch Command

Codemux launches Claude Code with:

```bash
claude --dangerously-skip-permissions --system-prompt "$CODEMUX_AGENT_CONTEXT" "your task description"
```

- `--dangerously-skip-permissions` — Runs Claude Code in autonomous mode without interactive permission prompts
- `--system-prompt "$CODEMUX_AGENT_CONTEXT"` — Injects context telling Claude Code to use Codemux's browser commands instead of system browsers
- The final argument is the user's task description from workspace creation

### CODEMUX_AGENT_CONTEXT

This environment variable is set on all Codemux terminal sessions. It contains instructions for agents about using Codemux's built-in browser commands. The `--system-prompt` flag references it so Claude Code receives this context at launch.

Users can edit agent presets in Settings > Presets, but should keep the `--system-prompt "$CODEMUX_AGENT_CONTEXT"` flag for best results.

### Hook Integration

Codemux registers hooks in `~/.claude/settings.json` on startup to track Claude Code's status in real-time. Three events are registered:

- `UserPromptSubmit` — Agent started working
- `Stop` — Agent finished
- `PermissionRequest` — Agent needs input

The hooks call `~/.codemux/hooks/notify.sh`, which sends an HTTP request to Codemux's local hook server. This powers the agent status indicators (amber/red/green dots in the sidebar and tab bar).

**If you see Claude Code settings errors**, check `~/.claude/settings.json` for Codemux entries under the `hooks` key. Codemux only modifies the `hooks` section and preserves all other settings.

The hooks are safe when Codemux isn't running — the notification script checks for required environment variables and silently exits if they're not set.

## Rules

1. **Never** open system browsers — use `codemux browser`
2. **Never** launch GUI apps — use Codemux built-in tools
3. The user can see everything you do in real-time
4. When asked to "test in browser" or "check the website", use `codemux browser open <url>`
5. Get a snapshot before interacting so you know what elements exist
6. Use explicit selectors — don't guess element presence
7. **Never** write website/marketing documentation in this repo — the public docs site (https://docs.codemux.org) lives in `~/projects/codemux-sitev2/` (Next.js + Fumadocs). New or updated doc pages go in `content/docs/` of that repo as `.mdx` files.

## Discovering Commands

```bash
codemux --help              # List all subcommands
codemux browser --help      # Browser subcommands
codemux capabilities        # JSON listing of all commands
```

All commands are also available via the Unix socket API at `$XDG_RUNTIME_DIR/codemux.sock`.
