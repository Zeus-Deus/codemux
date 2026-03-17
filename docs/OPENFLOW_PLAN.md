# OpenFlow Orchestration Plan

## Status: 🔄 Phase 6 In Progress

This document describes the implementation of **OpenFlow**, a multi-agent orchestration system where a swarm of AI coding agents collaboratively build, test, and review software based on a user prompt.

---

## ⚠️ CRITICAL: Modular Architecture

OpenFlow must be designed as a **standalone, embeddable orchestration engine**, NOT tightly coupled to Codemux.

### Design Principles
1. **Core is language-agnostic** - The orchestration logic should work for any type of agents (coding, business tasks, etc.)
2. **Clean boundaries** - Separate:
   - OpenFlow core runtime (Rust)
   - Agent adapters (how to spawn/communicate with different CLI tools)
   - Codemux integration layer (UI + terminal management)
3. **Extractable** - Should be usable as a framework anywhere

### Architecture
```
┌─────────────────────────────────────────────────────────────┐
│                     Codemux (Host)                          │
│  ┌─────────────────────────────────────────────────────┐   │
│  │            OpenFlow Codemux Integration              │   │
│  │  - UI components                                     │   │
│  │  - Terminal/workspace management                    │   │
│  │  - Browser integration                              │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                   OpenFlow Core (Rust)                       │
│  - Orchestration engine                                     │
│  - Run state machine                                        │
│  - Phase management                                        │
│  - Checkpoint/approval system                               │
│  - Persistence                                             │
└─────────────────────────────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                 Agent Adapters (Pluggable)                   │
│  - OpenCodeAdapter                                          │
│  - ClaudeAdapter                                            │
│  - CodexAdapter                                            │
│  - AiderAdapter                                            │
│  - CustomAdapter                                           │
└─────────────────────────────────────────────────────────────┘
```

---

## Current Implementation Status

### ✅ Working Features
- **Agent Spawning**: Multiple agents spawn in terminal panes with correct roles
- **Communication Log**: All agent output captured to `~/.local/share/.codemux/runs/<run_id>/communication.log`
- **Orchestrator**: Assigns tasks to agents, monitors DONE/BLOCKED messages
- **Builder Agent**: Creates files in the correct working directory
- **Tester Agent**: Uses browser automation (`codemux browser open`, `snapshot`, `click`, etc.)
- **RUN COMPLETE**: Orchestrator signals completion
- **User Injection**: User can send messages to orchestrator
- **Auto-orchestration**: Loop triggers every 10 seconds
- **Phase Transitions**: Working - Plan → Execute → Verify → Review → WaitingApproval
- **User Messages After Completion**: Detected and triggers workflow restart (phase goes back to Planning)
- **UI Improvements**: Timeline removed, orchestration view takes full screen
- **Browser Toggle Button**: Toggles between Orchestration view and Browser placeholder view
- **Run Cleanup Hardening**: OpenFlow session state is stripped on load, PTY respawn is capped, and stop flow removes agent sessions more aggressively
- **Dev Diagnostics**: Durable logs exist for wrapper lifecycle, OpenFlow breadcrumbs, native startup/panic/signal logging, and cwd-independent native launch attribution

### ⚠️ Known Issues (To Fix)
1. **Orchestrator doesn't directly respond to user questions** - When user sends message during/after run, orchestrator restarts workflow instead of answering simple questions directly
2. **Browser placeholder view only** - The "Browser" button toggles a placeholder view, not showing actual browser content from agents
3. **User messages during execution ignored** - Only triggers restart after reaching "awaiting_approval" phase
4. **Working directory** - Some agents may run in wrong directory

### 🔎 Diagnostics and Guardrails
- `.codemux/vite-wrapper.log` records the Vite wrapper lifecycle, child PID/PGID, and inbound signals such as `TERM`
- `.codemux/openflow-breadcrumbs.log` records run creation, agent spawning, run stop, and agent exits
- `.codemux/native-startup.log` records native GUI startup, run return, panic, and Unix signal attribution in debug builds
- `/run/user/$UID/codemux-native-launches.log` records every native launch even if the current working directory changes
- Bare `codemux` launches from OpenFlow agent sessions are explicitly blocked; agent terminals must use an explicit CLI subcommand such as `codemux browser open <url>`

---

## Vision

When a user opens an **OpenFlow workspace**, they enter a completely different experience from regular terminal workspaces:

1. **Setup Phase:** User configures how many agents to spawn and what CLI tools/models each should use
2. **Prompt Phase:** User provides the main goal (e.g., "build me a calendar booking site")
3. **Orchestration Phase:** A swarm of agents (orchestrator, planners, builders, reviewers, testers) work together
4. **Visual Monitoring:** User sees the orchestration in real-time, can inject instructions, and sees inter-agent communication

The browser pane is part of the verification - test agents use browser automation to verify code works.

---

## Key Concepts

### OpenFlow Workspace
A special workspace type that:
- Has a visual UI (not TUI-based)
- Manages multiple terminal sessions (one per agent)
- Shows agent hierarchy (orchestrator → workers)
- Has a right panel for inter-agent communication
- **Has a persistent browser pane** (always available for agents)

### Agent Types
- **Orchestrator** - Coordinates the workflow, assigns tasks, decides when to replan
- **Builder** - Writes code, implements features
- **Reviewer** - Reviews diffs, checks code quality
- **Tester** - Runs tests, uses browser automation for verification
- **Debugger** - Investigates failures, proposes fixes
- **Researcher** - Gathers context, searches docs

### Agent Configuration
Each agent needs:
- CLI tool: `opencode`, `claude`, `codex`, `aider`, or custom
- Model: dynamically discovered from the CLI tool
- Provider: dynamically discovered (e.g., `github-copilot`, `minimax-coding-plan`)
- Thinking mode: (for opencode) - dynamically discovered
- System prompt additions (optional)

---

## User Flow

### Step 1: Create OpenFlow Workspace
```
User clicks: "New Workspace" → Selects "OpenFlow Workspace"
```

### Step 2: Configure Agents
UI shows:
- Slider/dropdown: "How many agents?" (5, 10, 15, 20)
- For each agent (1-N):
  - Select CLI tool (auto-discovered from available tools)
  - Select model (auto-discovered from the tool)
  - Select provider (auto-discovered)
  - Select thinking mode (if applicable, auto-discovered)
  - Assign role (or auto-assign)

### Step 3: Provide Main Prompt
```
Text area: "What do you want to build?"
Example: "Build me a calendar booking site for a barbershop with React and Node.js"
```

### Step 4: Orchestration Begins
The UI shows:
- **Top:** Orchestrator status and current task
- **Visual Node Graph:** Agents as nodes connected by lines showing who is talking to whom
- **Right Panel:** Communication log (what agents are saying to each other)
- **Browser Button:** Toggle between Orchestration view and Browser placeholder

### Step 5: Monitor & Intervene
- Watch agents work in real-time via node graph
- See communication in right panel
- **Inject to orchestrator:** Type message in communication panel
- Click "Browser" button to toggle browser placeholder view
- Pause/resume/cancel run

---

## Architecture

### Frontend Components

```
OpenFlowWorkspace/
├── AgentConfigPanel.svelte      # Setup: how many agents, what tools
├── OrchestrationView.svelte      # Main UI with visual node graph + browser toggle
├── AgentNode.svelte             # Individual agent node in the graph
├── AgentEdge.svelte             # Connection lines between agents
├── CommunicationPanel.svelte    # Right panel: inter-agent chat + user input
└── NodeGraph.svelte            # Visual representation of agent network
```

### Backend Structure (Modular)

```
src-tauri/src/
├── openflow/
│   ├── mod.rs                   # Main orchestration engine (CORE - extractable)
│   ├── agent.rs                 # Agent configuration (CORE)
│   ├── orchestrator.rs          # Orchestrator logic (CORE)
│   ├── prompts.rs              # System prompts for each agent role
│   ├── communication.rs         # Inter-agent message passing (CORE)
│   ├── state.rs                # OpenFlow run state (CORE)
│   └── adapters/
│       ├── mod.rs              # Adapter trait (CORE)
│       └── opencode.rs         # OpenCode adapter
│
├── commands.rs                  # Tauri commands including spawn_openflow_agents
└── terminal/
    └── mod.rs                  # PTY spawning with correct working directory
```

### Agent Spawning
- Each agent = terminal session in a dedicated pane
- Terminal runs the CLI tool with configured model/provider
- Environment variables set:
  - `CODEMUX_AGENT_ROLE=builder`
  - `CODEMUX_OPENFLOW_RUN_ID=xxx`
  - `CODEMUX_COMMUNICATION_LOG=/path/to/log`
  - `CODEMUX_WORKING_DIR=/project/path`

### Communication Pattern
```
┌─────────────┐     Shared Memory      ┌─────────────┐
│ Orchestrator │ ◄─────────────────────►│   Builder   │
│   Agent     │    (project memory +   │   Agent     │
│  (terminal) │     run state file)    │  (terminal) │
└─────────────┘                        └─────────────┘
       │                                      │
       │         Shared Memory               │
       ▼                                      ▼
┌─────────────────────────────────────────────────────┐
│              Communication Log File                  │
│  (all agents read/write, UI reads for display)    │
│  Format: [TIMESTAMP] [ROLE] message               │
└─────────────────────────────────────────────────────┘
```

**User Injections:**
- User types message → written to communication log as `[user/inject]`
- Detected in auto-orchestration cycle → restarts workflow (goes to Planning phase)

---

## Implementation Phases

### Phase 1: OpenFlow Workspace Type & UI Shell
- [x] Add "OpenFlow Workspace" as workspace type in backend state
- [x] Create OpenFlowWorkspace.svelte component (different layout from regular workspace)
- [x] AgentConfigPanel.svelte for setup flow
  - [x] Dynamically discover available CLI tools
  - [x] Dynamically discover models per tool
  - [x] Dynamically discover thinking modes (where applicable)
- [x] Basic OrchestrationView.svelte with agent node graph

### Phase 2: Agent Spawning System
- [x] Agent config data structure (cli_tool, model, provider, role)
- [x] Agent adapter trait (for pluggable CLI tools)
- [x] OpenCodeAdapter implementation
- [x] Spawn terminal session with agent config
- [x] Set environment variables for agents
- [x] Track agent state per terminal session
- [x] Pass working directory to agents (via CODEMUX_WORKING_DIR)

### Phase 3: Communication Layer
- [x] Shared communication log file per run
- [x] Agent writes messages with format: `[TIMESTAMP] [ROLE] message`
- [x] CommunicationPanel.svelte polls/displays log
- [x] Inject command feature (user → orchestrator)
- [x] Auto-refresh communication panel
- [x] User messages detected - restarts workflow when in awaiting_approval

### Phase 4: Visual Node Graph
- [x] NodeGraph.svelte component (shows agent network)
- [ ] AgentNode.svelte component (individual nodes)
- [ ] AgentEdge.svelte component (connection lines)
- [ ] Highlight active communications
- [ ] Smooth animations

### Phase 5: Orchestrator Logic
- [x] System prompts for each agent role (orchestrator, planner, builder, reviewer, tester, debugger, researcher)
- [x] Agent adapter includes system prompt path via CODEMUX_SYSTEM_PROMPT_PATH env var
- [x] Wrapper script that reads prompt and passes to opencode
- [x] Orchestrator module with communication log analysis
- [x] Task assignment message generation
- [x] Phase advancement logic based on DONE/BLOCKED messages
- [x] Tauri command `trigger_orchestrator_cycle` to drive orchestration
- [x] Frontend "Orchestrate" button to trigger the cycle
- [x] Communication panel auto-refreshes every 2 seconds
- [x] Auto-orchestration loop every 10 seconds
- [x] RUN COMPLETE notification

### Phase 6: Browser Verification Integration
- [x] Test agents can call `codemux browser ...` commands
- [x] Browser pane created automatically on run start
- [x] Test agent captures screenshots as artifacts
- [x] Tester prompt includes browser commands
- [x] Browser button now toggles between Orchestration view and Browser placeholder view
- [x] Timeline removed for more visual space
- [x] Orchestration view takes full screen height

### Phase 7: Checkpoints & Approvals
- [ ] Define approval checkpoints (run start, major change, final apply)
- [ ] ApprovalModal.svelte for user interaction
- [ ] Pause/resume/cancel controls

### Phase 8: Context Management
- [ ] Implement context compaction for long runs
- [ ] Session restart capability (save state, restart agent, inject state)
- [ ] OpenFlow run memory (current state tracking)

### Phase 9: Persistence & Resume
- [ ] Save OpenFlow run state to disk
- [ ] Resume interrupted runs
- [ ] Load past runs in UI

### Phase 10: Documentation
- [ ] Write OpenFlow standalone documentation
- [ ] Document how to embed OpenFlow in other projects
- [ ] Document the adapter interface for new CLI tools
- [ ] Document the API for custom integrations

## 🛠️ Performance & Scalability Fixes (Critical)

### Crash Investigation: 20 Agents - RESOLVED ✅

**Symptom:** Running 20 agents crashes the Vite/Tauri dev server.
- ✅ 10 agents works fine
- ✅ 20 agents NOW WORKS (after fixes)

**Root Causes Found & Fixed:**
1. Orchestrator reading FULL log every cycle - Fixed with incremental reading
2. Terminal sessions never cleaned up - Fixed with proper cleanup
3. Frontend offsets never cleared - Fixed with cleanup on run end

---

### Test Results After Fixes

```
[DEBUG] Read 34 entries from comm log
[DEBUG] Read 25 entries from comm log
[DEBUG] Read 50 entries from comm log
[DEBUG] Read 5 entries from comm log
[DEBUG] Read 0 entries from comm log
[DEBUG] Read 0 entries from comm log
...
```

Entry counts stay SMALL and don't grow - incremental reading is working!

---

### Remaining Issues (Not Crashing But Can Improve)

1. **Duplicate app spawn** - Unknown cause, investigating
2. **High CPU usage** - 100% when agents active, see details below
3. **RAM usage** - ~26GB with 20 agents, acceptable
4. **GPU spikes** - Normal WebView rendering, not a problem

---

### CPU Usage Analysis

**Observation:** During 20-agent run, CPU hit 100% causing system lag.

**Analysis:**
- 20 agents = 20 terminal sessions = significant CPU usage
- Agents run `npm run dev`, `npm install`, build tools, etc.
- Each agent spawns 2 threads (reader + wait) = 40+ threads
- Orchestration runs every 10 seconds

**Is this normal?**
- Yes, for 20 parallel AI agents doing real work
- 100% CPU is expected when agents are active
- The lag is due to CPU contention

**Mitigation options:**
1. **Smart orchestration backoff** - When idle, increase delay from 10s to 30s
2. **Fewer agents** - 10 agents would use ~50% CPU
3. **Rate limiting** - Limit how many agents can run simultaneously
4. **CPU monitoring** - Add warnings when CPU exceeds threshold

**GPU Offloading:** Not recommended - the work is CPU-bound (terminal I/O, file operations), GPU won't help.

---

### Duplicate App Spawn Investigation

**Status:** Investigating - adding debug logging

**What we know:**
- Logs show agents run `codemux browser ...` commands (correct, via CLI, no window spawn)
- No code in app creates duplicate windows
- Could be: desktop environment issue, user action, or something else

**Debug approach:**
- Added logging to detect when app starts
- Will log process arguments to identify what's launching

**Updated diagnosis (2026-03-17):**
- `codemux browser ...` commands are **not** expected to launch a new Codemux GUI instance:
  - `codemux browser create` sends a control-socket request to the *running* app.
  - `codemux browser open/snapshot/click/fill/screenshot/console-logs` shells out to `npx agent-browser`, not to `codemux` GUI.
- The remaining plausible causes are:
  - **extra launch attempts** (from docs/tests/agents running `npm run tauri:dev`, bare `codemux`, or `cargo run -- ...`) which may exit quickly
  - a **race window** in the custom single-instance guard when the control socket is not bound yet (startup vs bind timing)
  - **stale socket replacement** causing confusing attribution (a socket can exist but not be alive)

**New durable logs and how to read them:**
- `/run/user/$UID/codemux-native-launches.log` now includes correlated lines for:
  - native process start (`startup_id`, `pid`, `cwd`, `argv`, `socket_existed`)
  - control socket lifecycle (`stale_socket_replace`, `bind_ok`, `bind_failed`)
  - Tauri lifecycle (`setup_enter`, `setup_exit`)
  - main window lifecycle (`component=window ... event=...`)
- If you see `outcome=single_instance_exit`, that process **attempted** to launch but exited before window creation.
- If you see `component=tauri event=main_window_available`, that process **reached** window creation (a real GUI instance).
- If you see multiple different `startup_id` values with `main_window_available` close together, that’s strong evidence of a real duplicate GUI spawn.

**Concrete fix direction (cross-platform safe):**
- **Layer 1: Codemux single-instance enforcement**
  - Replace the custom socket-only singleton logic with Tauri's official single-instance plugin.
  - Keep the control socket for CLI/browser IPC, but stop treating it as the source of truth for whether another GUI instance exists.
  - This is the cross-platform fix for duplicate top-level `Codemux` windows on Linux now and macOS/Windows later.
- **Layer 2: OpenFlow execution isolation**
  - Add a generic `ExecutionPolicy` / backend abstraction so OpenFlow can keep full capability while agent-run commands execute in an isolated environment.
  - Linux backend first: Bubblewrap-based sandbox with repo access, temp space, optional network, and no host desktop GUI access by default.
  - Future backends: macOS sandbox strategy and Windows restricted-process strategy, behind the same policy interface.
  - This is meant to be general for all future projects built in Codemux, not tied to any one framework or language.

**Planned implementation phases:**
1. **Immediate**
   - Integrate Tauri single-instance plugin and focus the existing window on duplicate launch attempts.
   - Keep the durable launch diagnostics while the new singleton path proves itself.
2. **Execution policy plumbing**
   - Add a generic execution policy to OpenFlow agent spawn specs.
   - Thread the policy through terminal spawning without changing agent capabilities yet.
3. **Linux backend**
   - Introduce a Linux sandbox backend for OpenFlow-spawned agent commands.
   - Preserve build/test/network/browser workflows while isolating them from the host desktop session.
   - Route bare `codemux` calls through a safe shim so agent sessions resolve to the active Codemux binary instead of an arbitrary older host install.
4. **Cross-platform follow-up**
   - Add macOS and Windows backends behind the same abstraction.
   - Keep OpenFlow runtime logic OS-agnostic.

---

### Comprehensive Issue List

#### CRITICAL - RESOLVED ✅

| # | Issue | Location | Status |
|---|-------|----------|--------|
| 1 | Orchestrator reads FULL log every cycle | `commands.rs:504` | ✅ Fixed with incremental reading |
| 2 | Backend state never cleaned up | `openflow/mod.rs` | ✅ Added cleanup methods |
| 3 | Terminal sessions never removed | `terminal/mod.rs` | ✅ Added cleanup on exit |
| 4 | Frontend offsets never cleared | `appState.ts` | ✅ Cleanup on run end |

#### HIGH - IN PROGRESS

| # | Issue | Location | Status |
|---|-------|----------|--------|
| 5 | Duplicate app spawn | Single-instance + execution isolation path | 🟡 Mitigated in one manual repro; needs more validation |
| 6 | High CPU usage (100%) | Optimization possible | 🔲 TODO |
| 7 | No orchestration backoff when idle | 🔲 TODO |

#### MEDIUM - TODO

| # | Issue | Location | Status |
|---|-------|----------|--------|
| 8 | Tauri event listeners never unregistered | `appState.ts` | 🔲 TODO |
| 9 | Log rotation TOCTOU race | `orchestrator.rs` | 🔲 TODO |

---

### Implementation Log

#### Phase 1: Critical Fixes - COMPLETED ✅

1. **Orchestrator incremental reading**
   - Added offset parameter to `trigger_orchestrator_cycle`
   - Backend now reads only NEW entries since last cycle
   - Frontend tracks offset and passes it
   - Result: Entry counts went from 33→60→90→121... (growing) to 34→25→50→5→0→0 (stable)

2. **Backend state cleanup**
   - Added `remove_run()` to OpenFlowRuntimeStore
   - Added `remove_for_run()` to AgentSessionStore  
   - Added terminal session cleanup on exit

3. **Frontend cleanup**
   - Added offset tracking for orchestrator cycles
   - Clear offsets when run ends

#### Phase 2: Investigating

- **Duplicate app spawn** - Agents may be spawning new instances
- **CPU optimization** - Could add backoff when idle

#### Phase 3: In Progress

- **Singleton hardening** - Tauri's official single-instance plugin is now wired in so duplicate launches can be redirected/focused before a second GUI window is created
- **Linux execution isolation** - OpenFlow agent spawns now select a Bubblewrap backend on Linux that keeps repo/network tooling usable while hiding host GUI/session sockets by default
- **Safe Codemux CLI routing** - agent sessions now prepend a `codemux` shim to PATH so bare `codemux ...` commands resolve back to the currently running Codemux binary instead of an arbitrary host install
- **Latest manual repro result** - one post-fix OpenFlow run (`openflow-run-54766AF5`) completed without spawning extra `Codemux` windows and without the previous `beforeDevCommand` teardown error in the captured terminal output
- **Remaining follow-up** - restart logs still show `Existing control socket ... appears stale; replacing it`, which did not break the successful repro but is still worth hardening so startup attribution stays clean

---

### Future Optimizations (Nice to Have)

1. **Smart orchestration backoff**
   - When no new entries, increase delay from 10s to 30s
   - When active, keep at 10s

2. **CPU throttling option**
   - Detect high CPU and reduce orchestration frequency
   - Add "low power mode" for resource-constrained devices

3. **GPU offloading**
   - Not recommended - work is CPU-bound, GPU won't help significantly

---

### Issue Categories

#### 1. Frontend Issues (HIGH Priority)

| Issue | Location | Impact |
|-------|----------|--------|
| Excessive polling (2s interval) | `OrchestrationView.svelte:98-112` | Heavy IPC overhead every 2 seconds |
| Heavy reactive derivations | `OrchestrationView.svelte:137-179` | O(n*m) complexity per update |
| Connection recalculation on every log | `OrchestrationView.svelte:181-276` | Full recalculation every cycle |
| No message virtualization | `CommunicationPanel.svelte:69-80` | Renders ALL messages (hundreds/thousands) |
| Auto-scroll thrashing | `CommunicationPanel.svelte:15-22` | Layout thrashing on rapid messages |
| Node position recalculation | `NodeGraph.svelte:38-91` | Recomputes all positions on any change |
| Store grows infinitely | `OrchestrationView.svelte:106-108` | Memory bloat - appends without limit |
| Orchestration too frequent | `OrchestrationView.svelte:64-70` | Every 10s = too aggressive with 20 agents |

**Frontend Fixes:**
- [x] Increase polling interval from 2s to 5s (or adaptive)
- [x] Limit CommunicationPanel to show only last 100 messages
- [x] Cache connection derivations until phase change
- [x] Memoize node positions (only recalc when node count changes)
- [x] Debounce auto-scroll, check if user is near bottom first
- [x] Add virtual scrolling for message list
- [x] Limit store to max 500 entries to prevent memory bloat
- [x] Reduce orchestration cycle from 10s to 20s

#### 2. Backend Concurrency Issues (HIGH Priority)

| Issue | Location | Impact |
|-------|----------|--------|
| Sequential blocking spawn loop | `commands.rs:348-379` | Agents spawn one-by-one, blocking |
| No concurrency limits | Global | No semaphore, can overwhelm system |
| 2 threads per agent | `terminal/mod.rs:827,881` | 20 agents = 40+ threads |
| No thread pooling | `terminal/mod.rs` | Each spawn creates new threads |
| Missing resource cleanup | `terminal/mod.rs:424-448` | Child processes not killed on close |
| No backpressure | `terminal/mod.rs:161-164` | Silently drops data when buffer full |

**Backend Fixes:**
- [ ] Add semaphore for concurrent agent spawning (limit to 8 at a time)
- [ ] Use tokio JoinSet for parallel agent spawn
- [ ] Add thread count tracking with warnings
- [ ] Implement proper Drop for SessionRuntime (kill child processes)
- [ ] Add backpressure signaling
- [x] Add delay between agent spawns (100ms) to prevent resource explosion

#### 3. Communication Log Race Conditions (HIGH Priority)

| Issue | Location | Impact |
|-------|----------|--------|
| No file locking | `terminal/mod.rs:856-862` | Race condition on concurrent writes |
| Rotation race condition | `orchestrator.rs:287,295-306` | Data loss during rotation |
| 500-line threshold too low | `orchestrator.rs:287` | Triggers too frequently with 20 agents |
| Full file read on every poll | `commands.rs:419` | Reads entire file (blocking) |
| No write buffering | `terminal/mod.rs:856-862` | Open/write/close on every chunk |

**Log System Fixes:**
- [x] Add file locking for concurrent writes
- [x] Increase rotation threshold from 500 to 5000 lines
- [x] Implement incremental reading (track offset, only read new content)
- [ ] Fix rotation race with exclusive locking
- [ ] Buffer writes (batch before writing)

#### 4. Future-Proofing

- [ ] Add agent count warnings (warn at 15+, recommend max)
- [ ] Adaptive resource management (reduce polling when agents > 10)
- [ ] Monitor system resources (log memory/thread warnings)
- [ ] Make agent limits configurable

---

## Next Steps (For New Chat Session)

### Priority 1: Fix Orchestrator Response to User Messages
The key issue: When user sends a message (either during execution or after completion), the orchestrator should:
1. **For simple questions** (e.g., "how do I run this?", "what did you build?") - Answer directly WITHOUT restarting workflow
2. **For modification requests** (e.g., "add feature X", "fix bug Y") - Restart workflow and assign tasks

Current behavior: Always restarts workflow regardless of what user asks.

**Files to investigate:**
- `src-tauri/src/openflow/orchestrator.rs` - `determine_next_phase()` function
- `src-tauri/src/commands.rs` - `trigger_orchestrator_cycle()` function
- The orchestrator prompt in `src-tauri/src/openflow/prompts.rs` - needs to explicitly tell orchestrator to respond to USER REQUEST messages

### Priority 2: Browser View Integration
- Current: Browser button toggles a placeholder view
- Desired: Show actual browser iframe/content from Codemux's browser pane
- This requires integrating the browser component into OrchestrationView

### Priority 3: User Message During Execution
- Currently only triggers restart when in `awaiting_approval` phase
- Should work during any phase so users can intervene mid-build

### Priority 4: Working Directory Verification
- Ensure all agents run in user-selected directory

---

## Debug Logging

To debug orchestration issues, check:
1. Terminal logs with `[DEBUG]` prefix (Rust backend)
2. Browser console with `[OpenFlow]` prefix (frontend)
3. Communication log at `~/.local/share/.codemux/runs/<run_id>/communication.log`

---

## Related Documents

- `docs/BROWSER_PLAN.md` - Browser automation for test agents
- `src-tauri/src/memory.rs` - Project memory implementation
- `src-tauri/src/openflow/prompts.rs` - System prompts for each role
- `AGENTS.md` - Codemux agent guide (browser automation reference)
