---
title: OpenFlow
description: Multi-agent orchestration for coordinated AI workflows with real-time communication and phase management.
---

# OpenFlow

OpenFlow is Codemux's multi-agent orchestration system. It coordinates multiple AI agents working together on a task, with an orchestrator managing delegation, communication, and phase transitions.

> **Note**: OpenFlow is functional in the backend but the React frontend is still being ported. Some features described here may not yet be available in the UI.

## How It Works

1. **Create a run** — Define a title, goal, and select which agent roles to use
2. **Configure agents** — For each role, choose a CLI tool (Claude Code, OpenCode), model, and provider
3. **Start** — The orchestrator analyzes the goal, creates a plan, and delegates tasks to agents
4. **Monitor** — Watch agents communicate in real-time via the communication log
5. **Intervene** — Inject messages to redirect agents, provide clarification, or approve phase transitions

## Agent Roles

| Role | Purpose |
|------|---------|
| Orchestrator | Manages the overall plan and delegates to other agents |
| Planner | Breaks down the goal into actionable steps |
| Builder | Writes code and implements features |
| Reviewer | Reviews code changes for quality and correctness |
| Tester | Writes and runs tests |
| Debugger | Investigates and fixes bugs |
| Researcher | Gathers information and explores approaches |

## Communication Protocol

Agents communicate through a structured message log. The orchestrator uses `ASSIGN:` messages to delegate tasks and tracks completion via `DONE:` markers. Users can inject messages that are delivered to the orchestrator.

## Phase Transitions

OpenFlow runs progress through phases:

- **Planning** — Orchestrator analyzes the goal and creates a plan
- **Active** — Agents are working on delegated tasks
- **Waiting Approval** — Orchestrator pauses for user input
- **Completed** — All tasks done, orchestrator outputs final summary

## Model Compatibility

| CLI + Model | Reliability |
|-------------|-------------|
| Claude Code (Claude models) | High — full protocol compliance |
| OpenCode (Claude models) | Good — generally reliable |
| OpenCode (non-Claude models) | Variable — auto-translator helps but some models struggle with the protocol |

## Stuck Detection

OpenFlow includes automatic stuck detection:

- **Probe** (~50s) — Sends a system probe if no agent activity
- **Rescue** (~60s active, ~90s planning) — Attempts recovery if stuck

User messages wake the orchestration loop immediately, bypassing the normal 5-second polling interval.
