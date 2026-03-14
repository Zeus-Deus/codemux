# OpenFlow Orchestration Plan

## Status: 🔄 Phase 4 Next (Phases 1–3 Complete)

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
│  - Phase management                                         │
│  - Checkpoint/approval system                               │
│  - Persistence                                              │
└─────────────────────────────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                 Agent Adapters (Pluggable)                   │
│  - OpenCodeAdapter                                          │
│  - ClaudeAdapter                                            │
│  - CodexAdapter                                             │
│  - AiderAdapter                                             │
│  - CustomAdapter                                            │
└─────────────────────────────────────────────────────────────┘
```

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
- Has an optional browser pane (toggleable)

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

### Available Models (Example from opencode)
```
opencode/big-pickle
opencode/gpt-5-nano
github-copilot/claude-sonnet-4
github-copilot/gpt-4o
minimax-coding-plan/MiniMax-M2.5
... (many more - should be dynamically fetched)
```

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

**Important:** Do NOT hardcode models/thinking modes. Dynamically discover them from the CLI tools at runtime.

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
- **Browser Toggle:** Button to show/hide browser pane (not all projects need it)

### Step 5: Monitor & Intervene
- Watch agents work in real-time via node graph
- See communication in right panel
- **Inject to orchestrator only:** `@instruct: try a different approach` (orchestrator decides how to incorporate)
- Approve/reject checkpoints
- Pause/resume/cancel run

---

## Architecture

### Frontend Components

```
OpenFlowWorkspace/
├── AgentConfigPanel.svelte      # Setup: how many agents, what tools
│                                 # (dynamically discovers available CLI tools, models, providers)
├── OrchestrationView.svelte      # Main UI with visual node graph
├── AgentNode.svelte             # Individual agent node in the graph
├── AgentEdge.svelte             # Connection lines between agents
├── CommunicationPanel.svelte    # Right panel: inter-agent chat
├── InjectCommand.svelte         # User input for injecting to orchestrator
├── TimelinePanel.svelte         # Run timeline and artifacts
├── BrowserToggle.svelte         # Show/hide browser pane button
└── ApprovalModal.svelte        # Checkpoint approvals
```

### Backend Structure (Modular)

```
src-tauri/src/
├── openflow/
│   ├── mod.rs                   # Main orchestration engine (CORE - extractable)
│   ├── agent.rs                # Agent configuration (CORE)
│   ├── orchestrator.rs         # Orchestrator logic (CORE)
│   ├── communication.rs       # Inter-agent message passing (CORE)
│   ├── state.rs                # OpenFlow run state (CORE)
│   ├── persistence.rs          # Save/restore runs (CORE)
│   └── adapters/
│       ├── mod.rs              # Adapter trait (CORE)
│       ├── opencode.rs         # OpenCode adapter
│       ├── claude.rs           # Claude CLI adapter
│       ├── codex.rs            # Codex adapter
│       └── aider.rs            # Aider adapter
│
├── codemux_integration/        # Codemux-specific (NOT CORE)
│   ├── workspace.rs            # OpenFlow workspace type
│   ├── terminal_spawn.rs      # Spawn terminals with agent config
│   └── browser.rs              # Browser integration for test agents
```

### Agent Spawning
- Each agent = terminal session in a dedicated pane
- Terminal runs the CLI tool with configured model/provider
- Environment variables set: 
  - `CODEMUX_AGENT_ROLE=builder`
  - `CODEMUX_OPENFLOW_RUN_ID=xxx`
  - `CODEMUX_COMMUNICATION_LOG=/path/to/log`

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
│  (all agents read/write, UI reads for display)      │
│  Format: [TIMESTAMP] [ROLE] message                 │
└─────────────────────────────────────────────────────┘
```

**User Injections:**
- User sends message → written to communication log
- Orchestrator reads it on next cycle → incorporates into planning
- This prevents breaking the loop

### Memory & Context Management

**Project Memory** (already implemented in `src-tauri/src/memory.rs`):
- Project brief, goal, focus, constraints
- Pinned context, decisions, next steps
- Session summaries
- Handoff packet generation

**OpenFlow Run Memory** (needs to be added):
- Current orchestration state
- What has been built so far
- Current phase and task status
- Artifacts produced
- Key decisions made during run

**Context Size Management:**
- Orchestrator reads summaries, not full agent outputs
- Periodic context compaction (summarize old messages)
- **Session restart capability:** If context gets too large:
  - Save current state to memory
  - Start fresh session for agent
  - Inject state from memory into new session
  - Continue without breaking the loop

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

> **⚠️ TODO: Thinking modes need model-specific platform support**
> 
> The current implementation hardcodes thinking modes per tool (e.g., opencode has auto/none/low/medium/high).
> However, different models support different thinking modes:
> - `opencode/big-pickle` supports: `high`, `max`
> - GPT models may support: `minimal`, `low`, `medium`
> 
> **Needed:**
> 1. Each adapter should expose `supported_thinking_modes(model_id) -> Vec<ThinkingModeInfo>` 
> 2. Query `opencode models --verbose` to get per-model variants with thinking budgets
> 3. Only show thinking mode dropdown when the selected model actually supports it
> 4. Display the *actual* applied thinking mode in the agent node (parse from CLI output or query model info)

### Phase 3: Communication Layer
- [x] Shared communication log file per run
- [x] Agent writes messages with format: `[TIMESTAMP] [ROLE] message`
- [x] CommunicationPanel.svelte polls/displays log
- [x] Inject command feature (user → orchestrator only)

### Phase 4: Visual Node Graph
- [ ] AgentNode.svelte component (shows agent name, role, status)
- [ ] AgentEdge.svelte component (lines connecting talking agents)
- [ ] Highlight active communications
- [ ] Smooth animations

### Phase 5: Orchestrator Logic
- [ ] Orchestrator agent has special system prompt
- [ ] Orchestrator reads other agents' outputs from communication log
- [ ] Orchestrator assigns tasks and decides next phase
- [ ] Phase loop: plan → execute → verify → review → (replan if needed)

### Phase 6: Browser Verification Integration
- [ ] Test agents can call `codemux browser ...` commands
- [ ] BrowserToggle.svelte to show/hide browser pane
- [ ] Test agent captures screenshots as artifacts

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

---

## Related Documents

- `docs/BROWSER_PLAN.md` - Browser automation for test agents
- `src-tauri/src/memory.rs` - Project memory implementation
- `PLAN.md` - Phase 11-13 cover OpenFlow runtime
- `PROJECT.md` - OpenFlow vision and architecture

---

## Next Steps

1. Implement Phase 1: OpenFlow workspace type and UI shell
2. Build AgentConfigPanel with dynamic discovery
3. Test basic agent spawning with 2-3 agents
4. Verify communication log works
5. Build visual node graph
6. Build out orchestrator logic
7. Integrate browser verification
8. Add approvals, context management, and persistence
