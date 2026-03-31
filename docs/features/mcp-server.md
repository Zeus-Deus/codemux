# MCP Server

- Purpose: Describe the built-in MCP (Model Context Protocol) server and the tools it exposes.
- Audience: Anyone working on agent integration, MCP tooling, or control surface expansion.
- Authority: Canonical MCP server feature doc.
- Update when: Tools are added/removed, transport changes, or auto-config behavior changes.
- Read next: `docs/reference/CONTROL.md`, `AGENTS.md`

## What This Feature Is

A JSON-RPC 2.0 MCP server that runs over stdio, exposing Codemux workspace, browser, pane, notification, and git tools to AI agents. Agents like Claude Code can connect to it as an MCP server and use the tools to control Codemux programmatically.

## Current Model

### Transport

- **Protocol**: JSON-RPC 2.0 over stdio (one JSON object per line)
- **Protocol version**: `2024-11-05`
- **Launch**: `codemux mcp` starts the server; agents connect via their MCP client config

### Auto-Configuration

On startup, Codemux can automatically write its MCP server config into `~/.claude/claude_desktop_config.json` and Claude Code's MCP settings so agents discover it without manual setup. This is controlled by the `auto_mcp_config` setting (default: enabled). Users can toggle it in Settings > Editor & Workflow > Agent.

### Tool Routing

Most tools delegate to the Codemux control socket (`$XDG_RUNTIME_DIR/codemux.sock`), reusing the same Rust helper implementations as the Tauri command layer and CLI. Git tools shell out to `git` in the workspace directory. The workspace is resolved from `CODEMUX_WORKSPACE_ID` env var.

## Tools (26)

### Browser — Tier 1: DOM-based (7)

| Tool | Description |
|------|-------------|
| `browser_navigate` | Navigate browser pane to a URL |
| `browser_snapshot` | Get interactive DOM elements with CSS selectors and bounding boxes |
| `browser_accessibility_snapshot` | Get the accessibility tree |
| `browser_click` | Click element by CSS selector |
| `browser_fill` | Fill input by CSS selector |
| `browser_screenshot` | Capture page as base64 PNG (includes viewport dimensions) |
| `browser_console_logs` | Get JavaScript console output |

### Browser — Tier 2: CDP/Vision-based (5)

| Tool | Description |
|------|-------------|
| `browser_click_at` | Click at x,y coordinates (left/right/double) |
| `browser_type_at` | Type text at optional x,y coordinates |
| `browser_scroll_at` | Scroll at x,y in a direction |
| `browser_key_press` | Send a key press (Enter, Tab, Escape, etc.) |
| `browser_drag` | Drag from start to end coordinates |

### Browser — Tier 3: OS-level Input (2)

| Tool | Description |
|------|-------------|
| `browser_click_os` | Click via ydotool (bypasses anti-bot) |
| `browser_type_os` | Type via ydotool (bypasses anti-bot) |

### Workspace (3)

| Tool | Description |
|------|-------------|
| `workspace_list` | List all open workspaces |
| `workspace_info` | Get active workspace details |
| `workspace_create` | Create a new workspace (optional path) |

### Pane (3)

| Tool | Description |
|------|-------------|
| `pane_list` | List panes in active workspace |
| `pane_split_right` | Split pane vertically |
| `pane_split_down` | Split pane horizontally |

### Notification (1)

| Tool | Description |
|------|-------------|
| `notify` | Send a notification (info or attention level) |

### Git (5)

| Tool | Description |
|------|-------------|
| `git_status` | Run `git status --porcelain` |
| `git_diff` | Run `git diff` (optional file path) |
| `git_stage` | Stage a file (or all with ".") |
| `git_commit` | Commit with a message |
| `git_push` | Push to remote |

## What Works Today

- Full MCP server with 26 tools over stdio transport
- Three-tier browser automation: DOM selectors, CDP coordinates, OS-level input
- Workspace and pane control for agent self-orchestration
- Git operations scoped to the agent's workspace directory
- Auto-configuration for Claude Code and Claude Desktop MCP settings
- Workspace resolution via `CODEMUX_WORKSPACE_ID` environment variable

## Current Constraints

- Stdio transport only (no HTTP or SSE transport)
- No streaming results (each tool call returns a single response)
- Git tools shell out to `git` rather than using the Rust git library
- OS-level input tools require `ydotool` to be available on the system
- Auto-config writes to Claude-specific config files; other agent platforms need manual setup

## Important Touch Points

- `src-tauri/src/mcp_server.rs` — MCP server implementation, tool registry, dispatch, socket bridge
- `src-tauri/src/cli.rs` — `codemux mcp` CLI entrypoint
- `src-tauri/src/agent_browser.rs` — DOM snapshot script used by `browser_snapshot`
- `src-tauri/src/control.rs` — Socket control server that MCP tools delegate to
- `src/stores/settings-store.ts` — `auto_mcp_config` setting
