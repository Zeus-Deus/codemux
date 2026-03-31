# Git Bot — AI-Powered Merge Conflict Resolution

- Purpose: Track remaining work and hardening priorities for the merge resolver.
- Audience: Anyone improving or validating the merge resolver.
- Authority: Active work plan, not current truth. See `docs/features/merge-resolver.md` for current capability.
- Update when: Priorities, open questions, or hardening targets change.
- Read next: `docs/features/merge-resolver.md`, `docs/core/STATUS.md`

## Goal

Provide a safe, AI-powered merge conflict resolution workflow that works on temporary branches, never touches real branches without explicit user approval, and integrates with the existing PR tab and Changes panel.

## Overview

A dedicated AI agent that resolves merge conflicts using a user-selected model. Works on temporary branches, never touches real branches without user approval.

## User Flow

1. User triggers merge (feature branch → main)
2. Conflicts detected → "Resolve with AI" button appears
3. Git Bot creates temp branch: `bot/merge-{source}-into-{target}`
4. Bot reads both sides of each conflict
5. Bot resolves keeping ALL functionality from both sides
6. Bot runs tests/CI if configured
7. Bot presents diff to user for review
8. User approves → merge applied. User rejects → temp branch deleted.

## Safety Rules

- Never force-push
- Never commit directly to main or target branch
- Always work on temporary branch
- User must explicitly approve before merge applies
- Full diff review required before approval
- If tests fail, flag and stop — don't auto-merge

## Settings

- Settings → Git → Git Bot section
- Model selector (Claude Opus, Sonnet, GPT-4, etc.)
- CLI tool selector (Claude Code, Codex, OpenCode)
- Auto-run tests toggle
- Default merge strategy (keep both, prefer incoming, prefer current)

## UI Location

- PR tab: "Resolve Conflicts" button when conflicts exist
- Changes panel: "Merge Assistant" when merge conflicts detected
- Dedicated tab showing bot progress (like OpenFlow agent view)
- Conflict review screen: side-by-side diff with bot's resolution

## Active Priorities

1. Design backend temp-branch creation and conflict detection commands
2. Design agent session integration (reuse OpenFlow infrastructure)
3. Design conflict review UI (side-by-side diff with approve/reject)

## Open Questions

- Should the bot spawn via OpenFlow agent sessions or a dedicated lighter-weight runner?
- How to handle multi-file conflicts — resolve all at once or file-by-file?
- Should the bot attempt rebasing as an alternative to merge when appropriate?
- How to present partial resolutions (some files resolved, some need human input)?

## Technical Requirements

- Backend: temp branch creation, merge execution, conflict detection
- Backend: spawn agent session with conflict context
- Frontend: conflict review UI with approve/reject
- Frontend: bot progress view
- Integration with existing OpenFlow agent session infrastructure

## Likely Touch Points

- `src-tauri/src/git.rs` — merge/conflict detection commands
- `src-tauri/src/github.rs` — PR conflict status
- `src-tauri/src/openflow/` — agent session spawning
- `src/components/workspace/pr-panel.tsx` — "Resolve Conflicts" button
- `src/components/workspace/changes-panel.tsx` — "Merge Assistant" entry point
- `src/tauri/commands.ts` — new command bindings
- `src/tauri/types.ts` — new types for bot state

## Future Extensions

- Auto-resolve on PR creation (run in background)
- Team rules (e.g., "always keep both, never delete code")
- Conflict resolution history (learn from past resolutions)
- Pre-merge CI integration

## Already Landed

- Temporary branch creation and cleanup (`create_resolver_branch`, `abort_resolution`)
- AI agent invocation for conflict resolution (`resolve_conflicts_with_agent`)
- Resolution diff generation and review (`get_resolution_diff`)
- Apply resolution with merge (`apply_resolution`)
- Frontend state machine with full lifecycle tracking (`ai-merge-store.ts`)
- Integration points in Changes panel and PR panel
- Settings for CLI tool and model selection

## Notes

- Current capability documented in `docs/features/merge-resolver.md`.
- Keep this file about remaining hardening work and open questions.
