# Branch Picker Redesign

- Purpose: Track implementation of the redesigned branch picker in the workspace creation flow.
- Audience: Anyone implementing or reviewing this work.
- Authority: Active work plan only, not current truth.
- Update when: Priorities, open questions, or likely touch points change.
- Read next: `docs/features/`, `docs/core/STATUS.md`

## Goal

Replace the cramped flat dropdown branch picker with a spacious, searchable panel featuring All/Worktrees tabs, per-branch metadata (timestamps, icons, badges), and distinct Open vs Create actions — matching the quality bar set by Superset while fitting our Tauri/worktree architecture.

## Research Summary

### What Superset Does

Superset's branch picker (in their `NewWorkspaceModal/PromptGroup`) has:

1. **Two tabs**: "All" (every branch) and "Worktrees" (only branches with an existing worktree on disk). Tab counts shown as badges.
2. **Three action states per branch**:
   - Active workspace exists → "Open" button (switches to it)
   - Worktree exists but no workspace → "Open" (reopens worktree) + "+ Create" (new worktree from branch)
   - No worktree → "+ Create" only
3. **Timestamps**: Last commit date via `git for-each-ref --sort=-committerdate --format=%(committerdate:unix)`, displayed as relative time (12h, 1d, 4d, 2mo).
4. **Search**: Substring match (`includes()`) in their main modal. A second dashboard modal uses Fuse.js fuzzy search.
5. **Icons**: Four states — active workspace (arrow), idle worktree (external link), local branch (git-branch), remote-only (globe).
6. **Layout**: 44px row height, monospace branch names at 12px, 12px horizontal padding, generous spacing between icon/name/timestamp/actions.
7. **Sorting**: Default branch first, then local before remote, then by last commit date (newest first).
8. **Data fetching**: Two-tier — fast local refs on open, optional background `git fetch --prune` for fresh remote data.

### What We Have Today

Our branch picker (`new-workspace-dialog.tsx:727-796`) is a shadcn `DropdownMenu`:

- **240×240px** max, flat alphabetical list, no search
- Backend `git_list_branches` returns **only branch names** — no timestamps, no commit metadata
- Three badges: "PR" (has open PR), "default" (main/master), "open" (workspace exists)
- Selecting a branch sets `baseBranch` for new worktree creation — no "Open existing" path
- Data already fetched on dialog open: local branches, remote branches, worktrees, PR list, current branch info

**What already works well (keep):**
- PR badge enrichment via `gh` CLI
- Default branch detection
- `branchWorkspaceMap` tracking which branches have open workspaces
- Worktree listing (`git_list_worktrees`)
- The overall workspace creation flow and dialog structure

## Active Priorities

1. New Rust command: `list_branches_detailed` returning per-branch metadata
2. New `BranchPicker` popover component replacing the dropdown
3. All/Worktrees tab filtering
4. Search input with substring matching
5. Open vs Create action buttons per branch row

## Implementation Plan

### Phase 1: Backend — Detailed Branch Listing

**Complexity: Low-Medium**

Add a new Rust function and Tauri command that returns rich branch metadata instead of bare names.

**New data structure** in `src-tauri/src/git.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchDetail {
    pub name: String,
    pub last_commit_unix: i64,     // committerdate as unix timestamp (seconds)
    pub is_local: bool,
    pub is_remote: bool,
}
```

**Why no `has_worktree`/`has_workspace`/`ahead`/`behind` fields?** Worktree and workspace status is frontend-derived state (from separate `listWorktrees` + `branchWorkspaceMap` calls already in the dialog). Mixing git ref data with app state in one backend call would couple concerns. Ahead/behind requires per-branch `rev-list` and is expensive at scale — compute it only for the selected branch, not the full list.

**New function** `git_list_branches_detailed(repo_path)`:

- Run `git for-each-ref --sort=-committerdate --format=%(refname:short)%09%(committerdate:unix) refs/heads/` for local branches
- Run `git for-each-ref --sort=-committerdate --format=%(refname:short)%09%(committerdate:unix) refs/remotes/origin/` for remote branches
- Parse each line: split on tab → `(name, timestamp)`
- Strip `origin/` prefix from remote names
- Merge into a deduplicated list: if a branch is both local and remote, set both flags, keep the local timestamp
- Sort: default branch first, then local-only before remote-only, then by `last_commit_unix` descending

**New Tauri command** in `src-tauri/src/commands/git.rs`:

```rust
#[tauri::command]
pub fn list_branches_detailed(path: String) -> Result<Vec<BranchDetail>, String> {
    crate::git::git_list_branches_detailed(Path::new(&path))
}
```

**New TypeScript type** in `src/tauri/types.ts`:

```typescript
export interface BranchDetail {
  name: string;
  last_commit_unix: number;  // unix epoch seconds — frontend formats
  is_local: boolean;
  is_remote: boolean;
}
```

**New command wrapper** in `src/tauri/commands.ts`:

```typescript
export async function listBranchesDetailed(path: string): Promise<BranchDetail[]> {
  return invoke<BranchDetail[]>("list_branches_detailed", { path });
}
```

**Existing `list_branches` — keep as-is, no deprecation.** It's used by 4 other callers that only need name lists:
- `changes-panel.tsx:1008` — remote branches for base-branch diff dropdown
- `pr-panel.tsx:128` — local branches for PR base selection
- `project-onboarding.tsx:115-116` — branch list for onboarding flow
- `branch_name.rs:77-80` — uniqueness check when generating branch names

These callers don't need timestamps or local/remote flags. No migration needed — the new `list_branches_detailed` supplements, it doesn't replace.

**Touch points:**
- `src-tauri/src/git.rs` — new function
- `src-tauri/src/commands/git.rs` — new command + register in handler
- `src-tauri/src/lib.rs:566` — add to handler registration list
- `src/tauri/types.ts` — new interface
- `src/tauri/commands.ts` — new wrapper

### Phase 2: Frontend — BranchPicker Component

**Complexity: Medium**

Extract the branch picker from the dialog into a standalone `BranchPicker` popover component.

**Component:** `src/components/overlays/branch-picker.tsx`

**Structure:**

```
Popover + PopoverContent (w-[360px], max-h-[420px])
├── Command (cmdk) wrapper — provides keyboard nav for free
│   ├── CommandInput (search, sticky top, text-xs, Search icon)
│   ├── Tab bar: "All (n)" | "Worktrees (n)" toggle
│   ├── CommandList (flex-1, overflow-y-auto)
│   │   ├── CommandEmpty — "No branches found"
│   │   └── CommandGroup
│   │       └── CommandItem (per branch row, h-11)
│   │           ├── Status icon (14px)
│   │           ├── Branch name (font-mono, text-xs, truncate)
│   │           ├── Relative timestamp (text-[11px], muted)
│   │           ├── Badges: "default", "PR"
│   │           └── Action buttons: "Open" / "Create"
│   └── Empty state for "Worktrees" tab when none exist
```

**Row height:** `h-11` (44px) — matches Superset's comfortable sizing, up from our current cramped rows.

**Popover trigger:** Same pill button style we have now (`rounded-full bg-muted/60`), but the popover content replaces the dropdown.

**Why `Popover` + `Command` (cmdk)?** The `ProjectPicker` (`project-picker.tsx`) already uses exactly this pattern inside the same dialog — `Popover > PopoverContent > Command > CommandInput > CommandList > CommandItem`. It proves:
- Popover-inside-Dialog works with Radix (portals to `<body>`, z-50, independent click-outside handling)
- `Command` gives us arrow-key navigation, search filtering, and enter-to-select for free
- No z-index conflicts — Radix Popover portals above the Dialog's z-layer automatically

**Click-outside behavior:** Radix handles layered dismissal correctly. Clicking outside the popover closes the popover but not the parent dialog. This already works for the ProjectPicker — no custom solution needed.

### Phase 3: Frontend — Tabs & Filtering

**Complexity: Low**

**"All" tab:**
- Shows every branch from `listBranchesDetailed` — local + remote-only, deduplicated
- Already sorted by the backend (default first, then recency)
- Count badge shows total branch count (e.g., "All 21")
- Does NOT exclude current branch, does NOT filter stale/merged branches
- Works with local refs only — no `git fetch`. The user sees what's in their local `.git/refs`. Add a "Fetch" button in v2 if this becomes a problem.

**"Worktrees" tab:**
- Filters to branches where a worktree exists on disk (cross-reference with `listWorktrees` results)
- OR where a workspace is currently open (`branchWorkspaceMap`)
- Shows count badge: number of matching branches (e.g., "Worktrees 3")
- Bare repo entry (main worktree with `is_bare: true`) is included if it has a workspace

**State:** `filterMode: "all" | "worktrees"` — resets to "all" when popover closes.

**Tab UI:** Inline toggle inside the popover, styled as `rounded-md bg-muted/40 p-0.5` with active state `bg-background text-foreground shadow-sm` (matches Superset's tab pattern, uses our semantic tokens).

### Phase 4: Frontend — Search & Keyboard Navigation

**Complexity: Low**

**Search:**
- `CommandInput` at top of popover, always visible, auto-focused on popover open
- cmdk handles filtering internally — `CommandItem` `value` prop matched against input
- Case-insensitive substring match (cmdk default behavior)
- Filters the current tab's branch list
- Clear button (X) when query is non-empty
- Searches within the active tab only (respects All/Worktrees filter)

**Not doing:** Fuzzy search (Fuse.js). cmdk's built-in substring matching is sufficient for branch names and avoids a dependency. Can revisit if repos with thousands of branches need it.

**Keyboard navigation** (provided by cmdk `Command` for free):
- Arrow Up/Down — navigate between branch rows (highlighted row tracks cursor)
- Enter — trigger primary action on highlighted row (Open if workspace/worktree exists, else select as baseBranch)
- Ctrl+Enter — trigger "Create" action on highlighted row (when both Open and Create are available)
- Escape — close the popover (Radix handles this)
- Type to search — input is always focused, typing filters immediately
- Tab — moves focus between "Open" and "Create" buttons when a row has both (standard focus order)

This matches Codemux's keyboard-first philosophy. The ProjectPicker already uses this exact cmdk pattern.

### Phase 5: Frontend — Icons & Timestamps

**Complexity: Low**

**Icons** (four states, checked in order):

| Condition | Icon | Color |
|-----------|------|-------|
| Workspace is open for this branch | `ArrowUpRight` (lucide) | `text-muted-foreground` |
| Worktree exists but no workspace | `FolderGit` (lucide) | `text-muted-foreground` |
| Local branch (no worktree) | `GitBranch` (lucide) | `text-muted-foreground` |
| Remote-only branch | `Globe` (lucide) | `text-muted-foreground` |

**Timestamps:**

- Format `last_commit_unix` as relative time: `now`, `5m`, `2h`, `3d`, `2mo`
- Utility function `formatRelativeTime(unixSeconds: number): string`
- Display right-aligned in the row, `text-[11px] text-muted-foreground/70`
- Show exact date on hover via `title` attribute

### Phase 6: Frontend — Open vs Create Actions

**Complexity: Medium**

This is the core UX improvement and the hardest design question. Each branch row shows contextual action buttons.

**Decision tree — 4 scenarios:**

```
Branch selected
│
├─ Has open Codemux workspace? (branchWorkspaceMap.has(name))
│  YES → Show [Open ↵] button only
│        Action: activateWorkspace(workspaceId), close dialog
│        Example: user has "feature/auth" workspace running
│
├─ Has worktree on disk but no workspace? (worktrees.find(wt => wt.branch === name) && !branchWorkspaceMap.has(name))
│  YES → Show [Open ↵] + [+ Create Ctrl+↵] buttons
│        Open: importWorktreeWorkspace(worktree.path, name, "single"), close dialog
│        Create: set baseBranch to this branch, keep dialog open for naming new branch
│        Example: user previously had a "fix/typo" workspace, closed it, worktree still on disk
│
├─ Is this the current branch of the project dir? (name === currentBranch)
│  YES → Show [Open ↵] button only
│        Action: createWorkspace(projectDir) (creates workspace on current checkout), close dialog
│        Example: user opens branch picker, clicks "main" which is already checked out
│        Note: This handles bare repos — main is the branch of the bare worktree.
│              The bare entry in listWorktrees has is_bare: true.
│              If a workspace already exists for main, the first case catches it.
│
└─ No worktree, not current branch (default for most branches)
   → Show [Create ↵] button only
     Action: set baseBranch to this branch, keep dialog open for user to name new branch
     (or proceed immediately if branch name already filled / auto-generated)
     Example: user picks "develop" as base → types "feature/new-thing" → Ctrl+Enter to create
```

**The `main` branch edge case:** In our bare-repo worktree setup, `main` appears in the worktree list with `is_bare: true`. If a workspace is open for `main`, it hits case 1 (Open). If no workspace but the worktree exists (bare entry), it hits case 2 (Open imports it / Create forks from it). If somehow no worktree entry (shouldn't happen for bare repo's branch), it falls through to "current branch" check or default. This matches the existing `handleSubmit` logic at `new-workspace-dialog.tsx:358-396`.

**Button styling:** `text-xs font-medium`, ghost variant. Buttons visible on row hover or when row is highlighted via keyboard. Keyboard hints shown as `text-[10px] opacity-60`.

**Integration with dialog:** 
- "Open" → closes dialog, activates/imports workspace immediately
- "Create" → keeps dialog open, sets `baseBranch`, focuses the branch name input for the user to type a new branch name. The dialog's existing Ctrl+Enter submit flow handles the rest.

**What the existing `handleSubmit` already does** (lines 338-396):
- Checks `branchWorkspaceMap` → activates existing workspace ✓
- Checks `currentBranch` → creates workspace on current checkout ✓
- Checks `worktrees` for orphan → imports worktree ✓
- Otherwise → `createWorktreeWorkspace` with `baseBranch` as parent ✓

The branch picker just surfaces these paths as explicit UI actions instead of hiding them behind the submit button.

### Phase 7: Wire Into New Workspace Dialog

**Complexity: Low**

Replace the `DropdownMenu` block at `new-workspace-dialog.tsx:727-796` with the new `<BranchPicker>` component.

**Props:**

```typescript
interface BranchPickerProps {
  projectDir: string;
  baseBranch: string;
  onSelectBase: (branch: string) => void;      // sets baseBranch for "Create"
  onOpenWorkspace: (workspaceId: string) => void; // switches to existing workspace
  onImportWorktree: (worktreePath: string, branch: string) => void; // imports orphan
}
```

**Data fetching:** Move the branch/worktree/PR fetching from the dialog's `useEffect` into the `BranchPicker` component (or a custom hook `useBranchList`). The dialog already fetches this data — we just relocate it.

**State cleanup in dialog:**
- Remove `localBranches`, `remoteBranches` state (replaced by `BranchDetail[]`)
- Keep `baseBranch`, `worktrees`, `prBranches`, `branchWorkspaceMap` — these are still needed
- Or encapsulate all of this inside `BranchPicker`

### Empty States

Three scenarios to handle:

1. **Search returns no matches** — `CommandEmpty` shows "No branches matching '{query}'" with muted text. Standard cmdk pattern.

2. **"Worktrees" tab has zero worktrees** — Show a centered message: "No worktrees yet. Create a workspace to start one." This is a valid state for fresh repos where the user hasn't created any workspaces.

3. **Repo has only `main`** — Show `main` as the single row. The "All 1" count makes the situation clear. No special handling needed — the picker still works, it's just a short list.

### Issue Picker Integration

The GitHub issue picker and branch picker are **independent, non-conflicting flows** in the dialog:

- **Issue picker** → sets `linkedIssue` and auto-fills `branchName` (the NEW branch name) via `suggestIssueBranchName()` (line 282-296). This is the name of the worktree branch to be created.
- **Branch picker** → sets `baseBranch` (the PARENT branch to fork from). This is which existing branch the new worktree is based on.

They don't interact. A user can link issue #42, get auto-filled branch name `feature/42-fix-auth`, AND pick `develop` as the base branch — all independently.

**Edge case:** If the auto-filled branch name happens to match an existing branch (e.g., someone already created `feature/42-fix-auth`), the submit handler at line 352-356 detects this via `allBranches.includes(resolvedBranch)` and sets `isNewBranch = false`, which checks out the existing branch instead of creating a new worktree. This is correct existing behavior and doesn't change.

**No auto-filter needed** — the branch picker doesn't need to highlight or filter based on the linked issue. The branch picker selects the *parent*, the issue picker names the *child*. These are separate concerns.

## What NOT to Copy From Superset

- **SQLite local database / Drizzle ORM** — we use Tauri AppState and git as source of truth, not a local DB for worktree tracking
- **TRPC architecture** — we use direct Tauri `invoke` calls
- **Two-tier fetch (local + background remote)** — unnecessary complexity for v1; a single `for-each-ref` on local refs is fast enough. Can add background `git fetch` later if needed
- **Fuse.js fuzzy search** — substring match is sufficient; avoids a new dependency
- **"Dashboard" second modal variant** — we have one workspace creation flow, keep it unified
- **`simple-git` library** — we already shell out to git via `run_git`, which works fine

## Open Questions

- **Orphan import agent/preset:** When "Open" imports an orphan worktree, should it use the dialog's currently selected agent/preset, or use defaults? Leaning toward defaults — the user is resuming existing work, not starting fresh. The current `importWorktreeWorkspace` takes only `(path, branch, layout)` with no agent param anyway.
- **List virtualization:** How many branches before we should virtualize? Probably 200+ — cmdk's `CommandList` already handles reasonable lists. Use `react-window` if scrolling becomes janky on large repos, but defer until it's a real problem.

## Likely Touch Points

- `src-tauri/src/git.rs` — new `git_list_branches_detailed` function
- `src-tauri/src/commands/git.rs` — new command + handler registration
- `src-tauri/src/main.rs` or `src-tauri/src/lib.rs` — register new command
- `src/tauri/types.ts` — new `BranchDetail` interface
- `src/tauri/commands.ts` — new `listBranchesDetailed` wrapper
- `src/components/overlays/branch-picker.tsx` — new component (the bulk of the work)
- `src/components/overlays/new-workspace-dialog.tsx` — replace dropdown with `<BranchPicker>`
- `src/components/overlays/new-workspace-dialog.test.tsx` — update tests

## Implementation Order

1. **Phase 1** (backend) — can be done independently, unblocks everything else
2. **Phase 2** (component skeleton) — basic popover with branch list rendering
3. **Phases 3-5** (tabs, search, icons/timestamps) — can be done in any order, low interdependency
4. **Phase 6** (Open vs Create actions) — depends on Phase 2 skeleton existing
5. **Phase 7** (wire in) — final integration, depends on all above

Phases 3, 4, and 5 can be parallelized or done in a single pass since they're all small and touch the same component.

## Already Landed

- Branch listing backend (`git_list_branches`) — works but returns names only
- Worktree listing backend (`git_list_worktrees`) — returns path + branch + is_bare
- `branchWorkspaceMap` in dialog — maps branches to open workspace IDs
- PR badge enrichment via `gh` CLI
- Workspace creation flow (dialog, form, worktree creation)

## Resolved During Plan Audit

These were raised as open questions during plan review and are now answered:

1. **Open vs Create decision tree** — fully specified in Phase 6 with 4 scenarios including the `main`/bare-repo edge case. Matches existing `handleSubmit` logic.
2. **Popover-inside-overlay z-index** — proven safe. `ProjectPicker` already uses `Popover > Command` inside the same `Dialog`. Radix portals to `<body>` at z-50, handles layered click-outside correctly.
3. **Data shape** — `BranchDetail` stays git-only (name, timestamp, is_local, is_remote). Worktree/workspace status derived on frontend from existing data. No ahead/behind in list (too expensive per-branch).
4. **`list_branches` collision** — no collision. Old command kept for 4 callers that only need names. New command adds metadata. No deprecation path needed.
5. **"All" tab count** — local + remote-only deduplicated. No exclusions. Local refs only, no `git fetch`.
6. **Keyboard nav** — cmdk `Command` provides arrow keys, enter, escape for free. Same pattern as `ProjectPicker`.
7. **Empty states** — 3 scenarios defined (no search matches, no worktrees, single-branch repo).
8. **Issue picker interaction** — independent flows. Issue sets `branchName` (child), picker sets `baseBranch` (parent). No cross-concern.

## Notes

- The `BranchPicker` should be a controlled popover, not routed through the overlay manager, since it lives inside the new-workspace dialog which is already an overlay.
- All colors must use semantic tokens (`text-muted-foreground`, `bg-muted`, etc.) — no hardcoded hex/oklch values.
- Row interactions: single click on a row should trigger the primary action (Open if available, else select as base branch). Action buttons provide explicit control.
- The `ProjectPicker` in `project-picker.tsx` is the reference implementation for "popover with searchable list inside a dialog" — follow its patterns closely.
