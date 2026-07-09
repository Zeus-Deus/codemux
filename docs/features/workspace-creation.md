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
7. **Model + reasoning selection** — a model pill beside the agent picker, shown when the chosen preset launches a CLI Codemux models (`claude` / `codex` / `opencode` / `gemini`). Picks a model and (Claude/Codex) a reasoning level; "Default" emits no flag

Creation dispatches one of:
- `createWorktreeWorkspace()` — new branch in worktree
- `importWorktreeWorkspace()` — existing orphan worktree
- `createWorkspace()` — standard workspace

**Base resolution for new branches**: before resolving the base ref, the backend runs a scoped, best-effort `git fetch origin <base>` (10s cap, `GIT_TERMINAL_PROMPT=0`) so the new branch starts from the latest remote commit, not a stale `origin/<base>` snapshot. Offline / no-remote repos fall back to the existing `origin/<base>` ref, else the local branch — creation never hard-fails on fetch problems. Both the desktop path (`src-tauri/src/git.rs::git_create_worktree`) and the headless daemon path (`src-tauri/src/remote/git.rs::create_worktree`) apply the same policy; the fetch runs on the blocking pool so the UI never freezes.

**Orphan worktree reclamation**: a worktree's conventional path is `~/.codemux/worktrees/<repo>/<branch>`. When a previous worktree for the same branch was removed from git's registry but its directory was left on disk (e.g. spinning up a workspace from an already-worked, often closed, issue branch whose `feature/<n>-<slug>` name is auto-suggested), `git worktree add` would otherwise die with `fatal: '<path>' already exists`. Creation now detects a **safe orphan** — a path git no longer tracks as a worktree, with no `.git` entry, holding nothing but Codemux's own `.codemux/` metadata (or empty) — and removes it before adding. Anything resembling user data, or a real path collision against a different registered branch, is left untouched so git still fails loudly. Both the desktop and headless daemon paths share this policy (`is_reclaimable_orphan_worktree`).

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
- Model + reasoning selection before launch (see "Model Selection" below)
- Manual branch name editing (stops auto-generation)
- GitHub issue linking with branch name suggestion and prompt context injection
- PR linking with branch auto-fill
- File attachment (paths appended to prompt) — staged as bordered chips (`workspace-attachment-chip.tsx`) with a per-type leading badge: image files show a real thumbnail (Tauri asset protocol, falling back to a glyph if it can't load), other files a file glyph, and clipboard pastes a friendly "Pasted image" label (full path on hover)
- Agent preset selection from pinned presets
- Package manager detection (npm, bun, pnpm, yarn, cargo, pip, uv, poetry, go, bundle, etc.)
- Setup/teardown script configuration with environment variables
- Orphan worktree detection and import
- Pending workspace tracking in UI during async creation
- Ctrl+Enter keyboard shortcut to create

## Model Selection

The model pill lets the user pick a model — and, where the CLI supports
them, a reasoning level and context window — before the agent launches,
instead of accepting the agent's default and restarting.

- **Detection.** The agent family is detected from the selected preset's
  command binary (`detectLaunchFamily` / Rust `detect_family`). Any
  preset launching `claude` / `codex` / `opencode` / `gemini` lights up
  the pill with no extra wiring; an unknown binary hides it.
- **Model source.** Claude / Codex / OpenCode read the shared
  `provider-capabilities-store` harvest — the same data as the Beta
  chat picker. All three are now fully live for every user:
  **Codex** via `codex app-server` `model/list` JSON-RPC; **OpenCode**
  via `GET /provider` against a managed `opencode serve` child; and
  **Claude** via a three-tier cascade — (1) `list-models` RPC against
  the claude-agent sidecar, which calls the SDK's
  `query.supportedModels()` and works for any Claude Code user
  regardless of auth (subscription, OAuth, API key) and surfaces the
  *deployed* CLI's actual effort vocabulary (including levels like
  `ultracode` the bundled SDK type union doesn't enumerate); (2)
  Anthropic's `GET /v1/models` REST API when `ANTHROPIC_API_KEY` is
  set; (3) the hand-maintained bundle. Sidecar runtime data merges
  with the maintained per-id metadata so context windows and the
  prompt-injected `ultrathink` keyword still surface. **Gemini** uses
  the same hybrid pattern via a backend `list_launch_gemini_models`
  Tauri command (`GEMINI_API_KEY` → Google's `generativelanguage`
  `models.list`, else the maintained fallback).
- **Reasoning.** Shown for the families whose CLI has a reasoning flag
  (`REASONING_FLAG_FAMILIES` — Claude `--effort`, Codex
  `-c model_reasoning_effort`). The *levels* are read live from the
  selected model's `effort_levels` in the capability bundle — so Sonnet
  shows its three levels and Opus its five, with nothing hardcoded.
  OpenCode TUI and Gemini have no reasoning flag, so the row is hidden.
- **Context window.** Claude-only, and fully capability-driven: the
  Context Window row lists the selected model's `context_window_options`
  (`200k` / `1M`). Picking 1M encodes it as a `[1m]` suffix on the model
  id (`claude --model 'claude-sonnet-4-6[1m]'`) — the same mechanism the
  Beta chat picker uses via `resolve_claude_api_model_id`. Before a
  concrete model is picked the default model's options stand in; if 1M
  is chosen with the model still on "Default", it resolves to the
  capability default model (and is dropped if that model can't do 1M).
- Reasoning levels and context options are derived per-model from the
  live capability bundle — nothing about the option lists is hardcoded.
- **Adaptive popover.** A flat list below `MODEL_SEARCH_THRESHOLD`
  models; search + star-favorites + sub-provider grouping above it.
- **Injection.** `agent_capability::apply_model_selection` splices the
  flags into the preset command before agent-context injection. An
  explicit pick strips any flag the preset already bakes in, so it
  never produces a duplicate. "Default" (the no-pick state) emits
  nothing — untouched workspaces are byte-identical to pre-feature.
- **Persistence.** The last pick per family is remembered in `ui-store`
  (`lastModelSelections`) and restored on reopen.

## Non-Git Projects

Any folder can be opened as a project — git is not required. This is a
deliberate, t3code-aligned UX decision: never refuse a folder, never
mutate it (`git init` only ever runs on an explicit user click), degrade
git-dependent features with honest messaging instead of silence.

### Model

- **Add paths never gate on git.** All entry points (folder picker /
  Ctrl+O, sidebar "+", chat drafts, onboarding) accept non-git folders
  silently. The old folder-picker `window.confirm` ("… is not a git
  repository. Initialize one?") that refused the project on decline was
  removed (`use-project-actions.ts`).
- **Plain-directory mode.** `create_workspace_impl` /
  `create_empty_workspace` resolve `project_root` via
  `find_git_root(...).unwrap_or(folder)` — the folder itself is the
  workspace root. Worktree creation intentionally hard-errors
  (`"Not a git repository"`), so the UI must not offer it (see below).
- **`is_git` snapshot flag.** `gather_workspace_git_info` stamps
  `WorkspaceSnapshot.is_git` using the same `find_git_root` predicate
  worktree creation uses. Serde-defaults to `true` (optimistic) so old
  persisted/synced snapshots render exactly as before until their first
  refresh; the error-path `WorkspaceGitInfo::default()` is also
  `is_git: true` so a transient gather failure never flashes the
  affordance on a real repo.
- **Explicit "Initialize Git" affordance** (`use-initialize-git.ts` —
  `showNoGitState` + `useInitializeGit`): shown only for LOCAL,
  STANDARD project workspaces with `is_git === false` (Home is not a
  project; `attach_only`/host-backed cwds are host paths the local
  probe can't judge). Clicking runs bare `git init` (no commit) then
  `refresh_workspace_git_info` so the git UI lights up immediately.

### Surfaces

- **Workspace context bar** — "Not a git repository" + Initialize Git
  button where the branch/kind cluster would render.
- **Context Row status cluster** (chat panes) — compact Initialize Git
  chip with an explanatory tooltip.
- **Changes panel** — "Not a git repository — changes can't be tracked"
  + Initialize Git, replacing the false "Working tree clean" (git
  errors were swallowed to `[]`).
- **Thread Scope row** — checkout + branch controls hidden for non-git
  projects (gated off the project's workspace snapshot); a stale
  "worktree" draft mode is snapped back to "current"; the scope hint
  says the run lands in the folder and how to unlock worktrees.

### Known remaining gaps

- `new-workspace-dialog.tsx` (branch modes) still assumes git; a
  non-git project going through it surfaces the backend error as a
  toast rather than hiding the branch UI.
- Checkpoints silently no-op for non-git (backend returns `Ok(None)`) —
  acceptable, matches t3code.
- After a bare `git init` (unborn HEAD) the branch may stay `None`
  until the first commit, so branch-gated UI stays hidden — also
  matches t3code.

## Current Constraints

- Layout selection not exposed in the dialog (defaults to single pane)
- No workspace templates or saved configurations
- No multi-issue linking (one issue per workspace)
- Package detection is best-effort (one pass on project open)
- Context-window selection is Claude-only — the other CLIs expose no
  context flag (their context window is fixed per model).
- Model selection is not yet wired into `project-onboarding.tsx` (the
  first-project flow) — only the New Workspace dialog.
- Native folder/file pickers on Linux need either a working XDG
  desktop portal (with a FileChooser backend such as
  xdg-desktop-portal-gtk) or zenity (issue #95). Every dialog command
  calls `dialog_preflight::select_backend` first, which probes the
  portal (4 s timeout) + `which zenity` and returns one of three
  backends: **Portal** (let rfd handle it), **Zenity** (the portal is
  unusable but zenity exists → Codemux drives zenity *itself* via
  `dialog_fallback.rs` with a sanitized env + 180 s timeout, because
  rfd's own zenity fallback only triggers on a portal *error*, not a
  *hang*, and would inherit the broken session env), or **None**
  (nothing can open a dialog). The None case rejects with a
  `NO_FILE_PICKER_BACKEND` marker carrying a cause-specific message
  (portal-not-installed vs installed-but-not-starting vs
  no-session-bus); `src/lib/file-dialog.ts` turns it into an
  actionable toast instead of silently doing nothing. `codemux doctor`
  prints the same diagnosis from a terminal.

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
- `src/components/overlays/launch-model-picker.tsx` — model + reasoning pill
- `src/lib/launch-models.ts` — family detection, reasoning tables, Gemini list
- `src-tauri/src/agent_capability.rs` — `ModelSelection`, `detect_family`, `apply_model_selection` flag injection
- `src-tauri/src/commands/presets.rs`, `workspace.rs` — `apply_preset` / `create_worktree_workspace` thread the selection through
- `src-tauri/src/dialog_preflight.rs` — `select_backend` / `diagnose` / `no_backend_remediation`: the Linux file-dialog backend decision (Portal / Zenity / None) + cause-specific remediation
- `src-tauri/src/dialog_fallback.rs` — driving `zenity` directly with a sanitized env + timeout when the portal is unusable but zenity exists
