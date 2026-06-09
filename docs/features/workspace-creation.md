# Workspace Creation

- Purpose: Describe the workspace creation flow and project onboarding.
- Audience: Anyone working on workspace lifecycle or onboarding UX.
- Authority: Canonical feature-level reality doc.
- Update when: Creation dialog, onboarding steps, or linking behavior changes.
- Read next: `docs/features/setup-teardown.md`, `docs/features/worktree-setup.md`, `docs/features/github-issues.md`

## What This Feature Is

Workspace creation is a multi-step flow that combines branch management, AI agent selection, issue linking, and project setup into a single dialog. A separate onboarding flow handles first-time project setup.

## Current Model

### New Workspace Dialog

The main creation dialog (`new-workspace-dialog.tsx`, ~870 lines) supports:

1. **Project selection** — choose or confirm project directory via ProjectPicker
2. **Task description** — textarea for "What do you want to do?" with auto-resize
3. **Branch mode** — create new branch (auto-named from prompt via AI) or open existing branch/worktree
4. **Attachments** — attach files or link a PR (auto-fills branch from PR head)
5. **Issue linking** — link a GitHub issue (auto-suggests branch name, injects issue context into prompt)
6. **Agent selection** — choose from pinned presets (defaults to last-used or Claude Code)

Creation dispatches one of:
- `createWorktreeWorkspace()` — new branch in worktree
- `importWorktreeWorkspace()` — existing orphan worktree
- `createWorkspace()` — standard workspace

**Base resolution for new branches**: before resolving the base ref, the backend runs a scoped, best-effort `git fetch origin <base>` (10s cap, `GIT_TERMINAL_PROMPT=0`) so the new branch starts from the latest remote commit, not a stale `origin/<base>` snapshot. Offline / no-remote repos fall back to the existing `origin/<base>` ref, else the local branch — creation never hard-fails on fetch problems. Both the desktop path (`src-tauri/src/git.rs::git_create_worktree`) and the headless daemon path (`src-tauri/src/remote/git.rs::create_worktree`) apply the same policy; the fetch runs on the blocking pool so the UI never freezes.

After creation: preset applied, issue linked, project marked as recent, workspace activated.

### Project Onboarding

Shown on first project open (`project-onboarding.tsx`, ~616 lines). Two steps:

**Step 1: Workspace Setup**
- Task input with auto-generated branch name (debounced 500ms)
- Base branch picker
- Banner for detected orphan worktrees with "Import all" option

**Step 2: Setup Script Configuration**
- **Checklist mode**: Auto-detected package manager actions (npm install, etc.) with toggleable checkboxes
- **Custom mode**: Manual setup/teardown command entry with environment variable hints
- **No detection**: Message with option to add custom commands or skip

After onboarding: scripts saved to project config, worktree workspace created, workspace activated.

## What Works Today

- AI-generated branch names from task description
- Manual branch name editing (stops auto-generation)
- GitHub issue linking with branch name suggestion and prompt context injection
- PR linking with branch auto-fill
- File attachment (paths appended to prompt)
- Agent preset selection from pinned presets
- Package manager detection (npm, bun, pnpm, yarn, cargo, pip, uv, poetry, go, bundle, etc.)
- Setup/teardown script configuration with environment variables
- Orphan worktree detection and import
- Pending workspace tracking in UI during async creation
- Ctrl+Enter keyboard shortcut to create

## Current Constraints

- Layout selection not exposed in the dialog (defaults to single pane)
- No workspace templates or saved configurations
- No multi-issue linking (one issue per workspace)
- Package detection is best-effort (one pass on project open)

## Important Touch Points

- `src/components/overlays/new-workspace-dialog.tsx` — main creation dialog
- `src/components/overlays/project-onboarding.tsx` — first-time onboarding flow
- `src/components/overlays/project-picker.tsx` — project directory selection
- `src/components/overlays/clone-dialog.tsx` — git clone flow
- `src/components/github/issue-picker.tsx` — issue linking UI
- `src/components/github/pr-picker.tsx` — PR linking UI (floating picker mirroring the issue picker; state-aware icons shared with the sidebar via `pr-status-icon.tsx`)
- `src-tauri/src/commands/workspace.rs` — workspace creation Tauri commands
- `src-tauri/src/commands/package_detect.rs` — package manager detection
- `src-tauri/src/branch_name.rs` — AI branch name generation
