# Merge Resolver

- Purpose: Describe the AI-powered merge conflict resolution feature.
- Audience: Anyone working on git integration, conflict resolution, or AI agent workflows.
- Authority: Canonical merge resolver feature doc.
- Update when: Resolver workflow, agent integration, safety model, or UI changes.
- Read next: `docs/plans/git-bot.md`, `docs/core/STATUS.md`

## What This Feature Is

An AI-powered merge conflict resolver that works on temporary branches. When merge conflicts are detected, the user can trigger an AI agent to resolve them. The agent works on a temp branch (`bot/merge-*`), never touching real branches without explicit user approval.

> **Status: currently unreachable from the UI.** The backend, the Tauri command surface, the frontend command wrappers, the zustand state machine, and the Settings config row all still exist and are wired to each other. What is missing is a **UI entry point** — the two buttons that called `startResolution` ("Resolve with AI" in the PR panel, "Merge Assistant" in the Changes panel) were removed by the refined-minimal UI pass (`92965c9`, "slim Changes panel + ADE-native right sidebar"), and `src/stores/ai-merge-store.ts` now has zero importers. Everything below describes a working system with no front door. Re-adding an entry point that calls `useAiMergeStore().startResolution(...)` should restore the whole flow; nothing else is known to be missing.

## Current Model

### Safety Rules

- All resolution work happens on a temporary branch
- Never force-pushes or commits directly to the target branch
- User must explicitly approve the resolution before it is applied
- Full diff review required before approval
- Rejecting a resolution deletes the temporary branch and restores the original state
- **Post-agent verification gate** (`ai::verify_resolution`): after the CLI exits, the backend independently verifies (a) `git status` shows zero `Conflicted` files AND (b) none of the originally-conflicting files contain `<<<<<<<` / `=======` / `>>>>>>>` markers. The UI never advances to "review" if the agent silently failed.
- **Branch-name de-recursion** (`git::strip_resolver_prefix`): retry from a stale `bot/resolve-*` branch peels the prior prefix instead of nesting (`bot/resolve-bot-resolve-...`).
- **Auto-cleanup on retry**: the frontend store calls `abortResolution` on any prior temp branch before starting a new one.
- **Non-interactive agent spawn** (`ai::build_resolver_argv`): the resolver is a one-shot headless task — there is no human to answer permission prompts. Every supported CLI is spawned with its skip-all-prompts flag so Edit/Write/Bash calls execute immediately:
  - `claude --print --dangerously-skip-permissions …`
  - `codex exec --full-auto …`
  - `opencode run --dangerously-skip-permissions …`
  This is safe because (a) the agent only ever operates inside the `bot/resolve-*` temp branch, (b) nothing is pushed or committed to the real branch without explicit user approval, and (c) the `verify_resolution` gate still runs post-hoc. The argv builder is a pure function with dedicated unit tests (`build_argv_*`) so a regression in any of these flag names fails CI immediately.
- **Spawn timeout** (`ai::RESOLVER_TIMEOUT`, 10 min): if the agent deadlocks anyway (e.g. stuck network call, future CLI version that reintroduces an interactive prompt despite the flag), the spawn is killed and the UI shows a clear "did not finish within Ns" error instead of spinning forever.
- **Hardened spawn helper** (`ai::run_resolver_cli`): every agent CLI spawn goes through a single helper that guarantees three non-negotiable invariants. Regressing any one of them reintroduces the "did not finish within 600s / stuck on an interactive prompt" failure mode:
  1. `stdin(Stdio::null())` — Claude Code (and similar CLIs) probe stdin and block indefinitely when they inherit an unusable parent stdin from the Tauri process (see anthropics/claude-code#43123 and #16306). `--print` / `--dangerously-skip-permissions` do NOT override this.
  2. `stdout(Stdio::piped())` / `stderr(Stdio::piped())` — explicit so the verification gate and error tail always have the agent's output.
  3. `kill_on_drop(true)` — when `tokio::time::timeout` fires and the `wait_with_output` future is dropped, the `Child` is dropped, SIGKILLing the spawned agent. Without this every "Try Again" would leak an orphan agent process that keeps running detached, accumulating zombies until the Codemux session exits.
  Regression tests (`run_resolver_cli_closes_stdin`, `run_resolver_cli_kills_child_on_timeout`, `run_resolver_cli_happy_path_captures_stdout`, `run_resolver_cli_surfaces_spawn_error_for_missing_binary`) drive the helper with `/bin/cat` and `/bin/sh` so the three invariants fail CI if anyone inlines the spawn or drops a flag.

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

The resolver has no enable toggle rendered in the UI. (`ai_resolver_enabled` exists as a stored settings key, defaulting to `"false"`, but `setAiResolverEnabled` has no caller in `src/components/`.) The design intent was that it only fires when the user explicitly clicks "Resolve with AI" on a real conflict, and even then runs on a temp branch with mandatory user review before apply, so gating it behind a setting was unnecessary friction.

Resolver settings are in Settings > Git > Merge Conflict Resolver (a `SectionGroup` under the Git section, sibling to `AI Tools`):
- Agent + model picker (Claude Code, Codex, OpenCode — the shared `MultiProviderModelPicker`)
- Default merge strategy (Smart merge / Keep both / Prefer mine / Prefer theirs)

## What Works Today

Present and wired to each other, but not reachable by a user:

- Temporary branch creation from conflict state
- AI agent invocation to resolve conflicts
- Resolution diff generation for review
- Approve/reject workflow with proper branch cleanup
- Frontend state machine tracking full resolver lifecycle (`ai-merge-store.ts`, no importers)
- Backend Tauri commands, all registered in `lib.rs`: `create_resolver_branch`, `resolve_conflicts_with_agent`, `apply_resolution`, `abort_resolution`, `get_resolution_diff`
- Frontend wrappers in `src/tauri/commands.ts` for all five
- Settings → Git → "Merge Conflict Resolver" (agent + model picker, default strategy). Note `ai_resolver_enabled` exists as a stored settings key but `setAiResolverEnabled` has no caller in the UI — there is no enable toggle rendered.

## Current Constraints

- **No UI entry point** — nothing in `src/components/` imports `ai-merge-store.ts` or calls `startResolution`. This is the single blocking gap; see the status note at the top.
- Single-agent resolution only (no multi-agent parallel resolution)
- No partial resolution support (all conflicts resolved at once or none)
- No automatic test running after resolution (manual verification required)
- Agent output is captured but not streamed live to the UI
- No conflict resolution history or learning from past resolutions
- Backend still runs single-shot resolution for all files; per-file streaming is the next planned improvement (see `docs/plans/git-bot.md`).

## Important Touch Points

- `src-tauri/src/git.rs` — `create_resolver_branch`, `apply_resolution`, `abort_resolution`, `get_resolution_diff`, `strip_resolver_prefix`, `has_conflict_markers`, `scan_files_for_conflict_markers`
- `src-tauri/src/ai.rs` — `resolve_conflicts_with_agent` (agent invocation), `run_resolver_cli` (hardened spawn helper: stdin-null + piped stdout/stderr + kill_on_drop), `verify_resolution` (post-agent gate), `generate_commit_message` (also routed through `run_resolver_cli` so a hung claude can't lock up the commit-message UI)
- `src-tauri/src/commands/git.rs` — Tauri command wrappers for resolver operations
- `src/stores/ai-merge-store.ts` — Frontend state machine (zustand); `startResolution` auto-cleans stale temp branches. **Currently has no importers.**
- `src/tauri/commands.ts` — Frontend command wrappers (all five still exported)
- `src/components/settings/settings-view.tsx` — "Merge Conflict Resolver" config row (`setAiResolverCli` / `setAiResolverModel` / `setAiResolverStrategy`)
- `src/components/workspace/changes-panel.tsx` — hosts plain git merge state (`getMergeState` / `abortMerge` / `continueMerge`, conflict badges). The former "Merge Assistant" resolver entry point lived here and is **gone**.
- `src/components/workspace/review-panel.tsx` — the former "Resolve with AI" entry point lived in its predecessor `pr-panel.tsx` and is **gone**.
- `src/components/ui/hover-card.tsx` — shadcn HoverCard wrapper, formerly used by the resolver progress UI
