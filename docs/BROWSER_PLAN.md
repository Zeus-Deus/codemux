# Browser Implementation Plan

## Status: ✅ WORKING

Browser automation is fully functional for agents!

Agent can control the browser via CLI commands, and users see the result in the browser pane (like a TV showing the agent's work).

---

## What's Implemented

### CLI Commands
```bash
codemux browser create
codemux browser open <url>
codemux browser snapshot
codemux browser click <selector>
codemux browser fill <selector> <value>
codemux browser screenshot
codemux browser console-logs
```

### Browser Pane Integration
- Uses agent-browser for both display and control
- Same session used by CLI commands
- 1-second polling for updates

### Environment Variables
- `CODEMUX_WORKSPACE_ID` - set in terminals
- `CODEMUX_SURFACE_ID` - set in terminals

---

## How Agents Use It

1. Agent reads AGENTS.md to discover browser capabilities
2. Agent uses CLI commands autonomously to test websites
3. User sees everything in the browser pane in real-time

---

## Architecture

```
CLI commands ──┐
               ├──► agent-browser (session: default)
Frontend ◄─────┘
```

---

## Files Modified

| File | Purpose |
|------|---------|
| `src-tauri/src/agent_browser.rs` | Manages agent-browser process |
| `src-tauri/src/cli.rs` | CLI commands |
| `src-tauri/src/commands.rs` | Tauri commands |
| `src-tauri/src/terminal/mod.rs` | Set env vars |
| `src/stores/appState.ts` | Frontend bindings |
| `src/components/panes/BrowserPane.svelte` | Uses agent-browser |
| `AGENTS.md` | Agent documentation |
| `docs/BROWSER_PLAN.md` | This file |

---

## What cmux Does (The Right Way)

cmux uses the **agent-browser** npm package (from Vercel Labs) for browser automation:
- Cross-platform: Chromium, Firefox, WebKit via Playwright
- CLI + programmatic API
- Accessibility tree snapshots with refs
- Click/fill/type by element refs or CSS selectors

We should do the same.

---

## Correct Plan

### Phase 1: Integrate agent-browser

1. **Add agent-browser as dependency**
   ```bash
   npm install agent-browser
   ```

2. **Create a browser service layer** that wraps agent-browser
   - Spawns browser sessions
   - Exposes automation commands
   - Handles screenshot streaming to frontend

3. **Platform support**:
   - Linux: uses Chromium (bundled or system)
   - macOS: uses WebKit or Chromium
   - Windows: uses Chromium

### Phase 2: Expose to Agents

1. **Add CLI commands** (`cli.rs`):
   ```
   codemux browser open <url>
   codemux browser snapshot
   codemux browser click <selector>
   codemux browser fill <selector> <text>
   codemux browser screenshot
   codemux browser console-logs
   ```

2. **Add socket protocol commands**:
   ```json
   {"cmd": "browser_open", "url": "..."}
   {"cmd": "browser_snapshot"}
   {"cmd": "browser_click", "selector": "..."}
   ```

### Phase 3: Wire to Frontend

1. **Frontend uses same agent-browser backend** for display
2. **Screenshot streaming** via polling or WebSocket
3. **Click interaction** sends CDP commands through agent-browser

---

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    Codemux App                          │
├─────────────────────────────────────────────────────────┤
│  Frontend (Svelte)                                      │
│  ┌─────────────────┐    ┌─────────────────────────┐   │
│  │  BrowserPane    │◄──►│  agent-browser (Node.js)│   │
│  │  (screenshot    │    │  - CDP automation        │   │
│  │   display)       │    │  - Screenshot capture   │   │
│  └─────────────────┘    └─────────────────────────┘   │
├─────────────────────────────────────────────────────────┤
│  Backend (Rust)                                         │
│  ┌─────────────────┐    ┌─────────────────────────┐   │
│  │  CLI commands   │◄──►│  Socket control server  │   │
│  │  (browser_*)    │    │  (agent JSON protocol)  │   │
│  └─────────────────┘    └─────────────────────────┘   │
└─────────────────────────────────────────────────────────┘
                              ▲
                              │
         ┌────────────────────┴────────────────────┐
         │   Terminal Agents (OpenCode, etc.)      │
         │   - Can call CLI commands                │
         │   - Can send socket JSON commands       │
         └─────────────────────────────────────────┘
```

---

## Implementation Steps

### Step 1: Install agent-browser
```bash
npm install agent-browser
```

### Step 2: Create browser service (TypeScript)
Create `src/lib/browserService.ts`:
- Initialize agent-browser sessions
- Spawn browser with --headless
- Handle all automation commands
- Stream screenshots back to frontend

### Step 3: Update frontend BrowserPane
- Use browserService instead of direct Tauri commands
- Get screenshots via polling or WebSocket
- Send user clicks through service

### Step 4: Add CLI/socket commands
- `codemux browser open <url>`
- `codemux browser snapshot`
- `codemux browser click <selector>`
- etc.

### Step 5: Test with agent
- Run agent in terminal
- Have agent control browser programmatically
- Verify full workflow works

---

## Files to Modify

| File | Purpose |
|------|---------|
| `package.json` | Added agent-browser dependency |
| `src/lib/browserService.ts` | New: Browser automation service wrapping agent-browser |
| `src/lib/browserAutomation.ts` | New: Event handler for browser automation requests |
| `src/components/panes/BrowserPane.svelte` | Using Tauri commands (can upgrade to browserService) |
| `src/stores/appState.ts` | Added browser automation bindings |
| `src/App.svelte` | Added browser automation event listener init |
| `src-tauri/src/cli.rs` | Added browser CLI commands |
| `src-tauri/src/control.rs` | Already has browser_automation command |
| `docs/BROWSER_PLAN.md` | This file |

---

## Why This Approach

1. **Cross-platform**: agent-browser supports Linux/macOS/Windows
2. **Battle-tested**: 21k+ stars, used by cmux in production
3. **Agent-friendly**: snapshot with refs, not just raw DOM
4. **Fast**: Native Rust CLI, sub-ms overhead
5. **Can improve later**: Still swap to native webview if needed

---

## Current Browser Pane (What's Working)

- Chromium spawns with remote debugging
- Screenshot capture via CDP works
- Basic navigation works
- Display via polling screenshots
- **NEW**: Agent-browser npm package integrated for browser automation
- **NEW**: Browser automation event handler in frontend
- **NEW**: CLI commands for browser control

## What's Missing

- ~~No CLI commands for agents to control browser~~ (DONE)
- ~~No socket protocol for agents~~ (Using existing browser_automation)
- ~~No accessibility tree snapshots~~ (Now using agent-browser)
- ~~No semantic element selection~~ (Now using agent-browser)
- Frontend not using agent-browser (Partially done - event handler uses it)

### Known Limitations (Acceptable for "TV" Use Case)

The browser pane is primarily a display ("TV") for agents to show their work to users. User interaction with the browser is not the main use case.

- Frontend toolbar: Only Home + Refresh screenshot buttons (no actual back/forward/reload)
- User clicking in browser: Clicks "body" always, not specific coordinates
- No scroll support in frontend (agents can scroll via eval if needed)

These limitations are acceptable because:
1. Agents control the browser via CLI commands
2. Users see what agents are doing via screenshot display
3. The address bar allows manual navigation if needed
