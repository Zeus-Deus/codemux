# Git Bot — AI-Powered Merge Conflict Resolution

- Purpose: Track remaining work and hardening priorities for the merge resolver.
- Audience: Anyone improving or validating the merge resolver.
- Authority: Active work plan, not current truth. See `docs/features/merge-resolver.md` for current capability.
- Update when: Priorities, open questions, or hardening targets change.
- Read next: `docs/features/merge-resolver.md`, `docs/core/STATUS.md`

## Goal

Provide a safe, AI-powered merge conflict resolution workflow that works on temporary branches, never touches real branches without explicit user approval, and integrates with the existing PR tab and Changes panel.

## Active Priorities

1. Deepen test coverage with real multi-file conflict scenarios
2. Live validation on real merge workflows across different project types
3. Improve partial resolution UX (some files resolved, some need human input)

## Open Questions

- Should the bot spawn via OpenFlow agent sessions or a dedicated lighter-weight runner?
- How to handle multi-file conflicts — resolve all at once or file-by-file?
- Should the bot attempt rebasing as an alternative to merge when appropriate?
- How to present partial resolutions (some files resolved, some need human input)?

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
