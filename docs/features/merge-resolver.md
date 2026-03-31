# Merge Resolver

- Purpose: Describe the AI-powered merge conflict resolution feature.
- Audience: Anyone working on git integration, conflict resolution, or AI agent workflows.
- Authority: Canonical merge resolver feature doc.
- Update when: Resolver workflow, agent integration, safety model, or UI changes.
- Read next: `docs/features/openflow.md`, `docs/core/STATUS.md`

## What This Feature Is

An AI-powered merge conflict resolver that works on temporary branches. When merge conflicts are detected, the user can trigger an AI agent to resolve them. The agent works on a temp branch (`bot/merge-*`), never touching real branches without explicit user approval.

## Current Model

### Safety Rules

- All resolution work happens on a temporary branch
- Never force-pushes or commits directly to the target branch
- User must explicitly approve the resolution before it is applied
- Full diff review required before approval
- Rejecting a resolution deletes the temporary branch and restores the original state

### Workflow

1. User triggers merge and conflicts are detected
2. "Resolve with AI" action creates a temporary branch via `create_resolver_branch`
3. AI agent (via configured CLI tool and model) resolves conflicts on the temp branch
4. Resolution diff is generated for review via `get_resolution_diff`
5. User reviews and either approves (`apply_resolution`) or rejects (`abort_resolution`)
6. On approval: resolution is merged. On rejection: temp branch is deleted.

### State Machine

```
idle → creating_branch → resolving → review → applying → idle
                                       ↓
                                     error
```

### Configuration

Resolver settings are in Settings > Editor & Workflow > Agent:
- CLI tool selector (Claude Code, Codex, OpenCode)
- Model selector
- Merge strategy preferences

## What Works Today

- Temporary branch creation from conflict state
- AI agent invocation to resolve conflicts
- Resolution diff generation for review
- Approve/reject workflow with proper branch cleanup
- Frontend state machine tracking full resolver lifecycle
- Backend Tauri commands: `create_resolver_branch`, `resolve_conflicts_with_agent`, `apply_resolution`, `abort_resolution`, `get_resolution_diff`
- Integration with Changes panel and PR panel

## Current Constraints

- Single-agent resolution only (no multi-agent parallel resolution)
- No partial resolution support (all conflicts resolved at once or none)
- No automatic test running after resolution (manual verification required)
- Agent output is captured but not streamed live to the UI
- No conflict resolution history or learning from past resolutions

## Important Touch Points

- `src-tauri/src/git.rs` — `create_resolver_branch`, `apply_resolution`, `abort_resolution`, `get_resolution_diff`
- `src-tauri/src/ai.rs` — `resolve_conflicts_with_agent` (agent invocation)
- `src-tauri/src/commands/git.rs` — Tauri command wrappers for resolver operations
- `src/stores/ai-merge-store.ts` — Frontend state machine (zustand)
- `src/tauri/commands.ts` — Frontend command wrappers
- `src/components/workspace/changes-panel.tsx` — "Merge Assistant" entry point
- `src/components/workspace/pr-panel.tsx` — "Resolve Conflicts" entry point
