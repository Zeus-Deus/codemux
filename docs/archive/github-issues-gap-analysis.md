# GitHub Issues Integration — Gap Analysis

- Purpose: Gap analysis and implementation plan for GitHub issue integration in workspace creation.
- Audience: Anyone implementing or reviewing the issue linking feature.
- Authority: Active work plan only, not current truth.
- Update when: Priorities, open questions, or likely touch points change.
- Read next: `docs/features/pr-integration.md`, `docs/core/STATUS.md`

## Goal

Add GitHub issue integration to Codemux's workspace creation flow — letting users browse, search, and link open issues to new workspaces. The linked issue provides context for the branch name, agent prompt, and ongoing workspace metadata. Modeled after Superset's implementation but tailored to Codemux's architecture.

---

## Section 1: What Superset Has (Issue Integration Features)

Superset (github.com/superset-sh/superset) ships GitHub issue integration as of PR #2678 (March 2026). Here is every issue-related capability:

### Source Files (Verified)

| Component | Path |
|---|---|
| Issue picker popover | `apps/desktop/src/renderer/components/NewWorkspaceModal/components/PromptGroup/components/GitHubIssueLinkCommand/GitHubIssueLinkCommand.tsx` |
| LinkedIssue type | `apps/desktop/src/renderer/components/NewWorkspaceModal/NewWorkspaceModalDraftContext.tsx` |
| tRPC router (listIssues, getIssueContent) | `apps/desktop/src/lib/trpc/routers/projects/projects.ts` (~line 414, ~line 471) |
| Workspace DB schema (no issue fields) | `packages/local-db/src/schema/schema.ts` |
| Chat issue linker (separate feature) | `apps/desktop/src/renderer/components/Chat/ChatInterface/components/IssueLinkCommand/IssueLinkCommand.tsx` |

### Issue Fetching
- Uses `gh issue list --state open --limit 30 --json number,title,url,state,labels` exclusively (no direct API calls, no SDK)
- The `--limit 30` is **hardcoded** as a literal `"30"` in the tRPC router args array (~line 425) — no constant or config. Matches the `gh` CLI default but is passed explicitly.
- Uses `gh issue view <number> --json number,title,body,url,state,author,createdAt,updatedAt` for full issue content
- Relies on user's existing `gh auth login` session — no separate OAuth flow
- 10-second timeout per fetch; 50KB body truncation
- Graceful degradation: if `gh` is not installed or not authenticated, issue features hide without breaking workspace creation

### Issue Search/Filter UI
- Component: `GitHubIssueLinkCommand` — shadcn `Command` (cmdk) popover
- Client-side fuzzy search via Fuse.js: `new Fuse(issuesWithSearchField, { keys: [{ name: "issueNumberStr", weight: 3 }, { name: "title", weight: 2 }], threshold: 0.4, ignoreLocation: true })` — note issue number is pre-stringified as `issueNumberStr` for searchability
- Supports direct URL pasting — if search matches an issue URL, returns that single result
- `MAX_RESULTS = 20` (hardcoded constant), fixed `max-h-[280px]` scrollable list (no virtual scroll, no pagination)
- Per-issue row shows: state icon (`LuCircleDot` green for open, `LuCircleCheck` purple for closed), `#number` in mono, truncated title, "Link (return)" hint on hover
- Does NOT show: labels (fetched but not displayed), assignees, body preview, creation date, comment count

### Issue-to-Worktree Linking
- Selecting an issue adds a `LinkedIssue` object to draft context: `{ slug: "#123", title, source: "github", url, number, state: "open"|"closed" }`
- Visual confirmation: `LinkedGitHubIssuePill` shows below prompt input with title (truncated to 180px), `#number`, "GitHub" label, state icon, and X to remove
- First linked issue's slug becomes the `taskSlug` parameter in agent launch
- Full issue body fetched at creation time, sanitized (HTML entities), converted to markdown file `github-issue-{number}.md`, base64-encoded, and attached to agent launch as an `initialFiles` entry
- Supports multiple linked issues per workspace
- Does NOT auto-name branch from issue (uses random `friendly-words` pattern: `{prefix}/{predicate}-{object}`)
- Does NOT auto-assign issue on GitHub or update issue status

### Ongoing Issue State
- No ongoing issue display on workspace sidebar row after creation
- No persistent storage of linked issues in workspace DB — the `workspaces` table has: `id, projectId, worktreeId, type, branch, name, tabOrder, createdAt, updatedAt, lastOpenedAt, isUnread, isUnnamed, deletingAt, portBase, sectionId`. No issue fields.
- No unlink mechanism post-creation
- No bi-directional sync (does not close issues on merge, does not track state changes)
- Separate in-chat feature lets users link issues during active sessions via `@task:slug` references (uses local task DB, not GitHub API)

### Architecture
- Data flow: React → electronTrpc (IPC) → tRPC router → `execWithShellEnv("gh", ...)` → JSON → Zod parse → TanStack Query cache
- No backend caching of issue data; frontend uses TanStack Query (conditional fetch — only runs when popover opens and `projectId` exists)
- Zustand for UI state, React Context (`NewWorkspaceModalDraftProvider`) for draft form state (including `linkedIssues[]`)
- All `gh` calls wrapped in try/catch with graceful degradation — failures return empty arrays, do NOT block workspace creation
- Issues treated as ephemeral creation-time context, not persistent workspace metadata

---

## Section 2: What Codemux Has Today

### GitHub CLI Infrastructure
- `gh` CLI integration via `std::process::Command` in `src-tauri/src/github.rs`
- Wrapper functions: `run_gh()`, `run_gh_json()`, `run_gh_optional()` with JSON parsing via serde
- Auth detection: `GhStatus` enum (`NotInstalled`, `NotAuthenticated`, `Authenticated { username }`)
- `gh_available()` checks PATH, `check_gh_status()` parses `gh auth status` stderr
- `check_github_repo()` verifies repo is GitHub-hosted via `gh repo view`

### PR Integration (Full Lifecycle)
- `PullRequestInfo` struct: number, url, state, title, head/base branch, draft, mergeable, additions/deletions, review_decision, checks_passing
- Full PR lifecycle: create, view, list, merge, checks, reviews (review-level + inline), deployments
- Frontend: `PrPanel` with sub-components for header, checks, reviews, review actions, deployments, merge controls
- Background refresh loop (every 5s) updates `pr_number`, `pr_state`, `pr_url` on active workspace
- PR linking in workspace creation dialog: fetches open PRs, clicking a PR pre-fills branch name

### Workspace Creation
- `create_worktree_workspace()` takes: `repo_path`, `branch`, `new_branch`, `base`, `layout`, `initial_prompt`, `agent_preset_id`
- Flow: `git worktree add` → create workspace with layout → set worktree metadata → spawn PTY → run setup scripts → generate `.mcp.json`
- Worktree path: `~/.codemux/worktrees/{repo_name}/{sanitized_branch}/`
- Frontend dialog: branch name, prompt textarea, base branch selector, agent preset picker, draft checkbox, attachment picker, PR linking dropdown

### Workspace Metadata
- `WorkspaceSnapshot` includes: workspace_id, title, cwd, git_branch, git stats (ahead/behind/additions/deletions/changed_files), worktree_path, project_root, pr_number, pr_state, pr_url
- State persisted via debounced disk writes
- No issue-related fields exist

### What Does NOT Exist
- No `GithubIssue` type or struct
- No `gh issue` CLI invocations anywhere
- No issue fetching, listing, or viewing commands
- No issue UI components
- No issue field in workspace state
- No issue commands in Tauri IPC layer
- No issue commands in control socket protocol

---

## Section 3: Gap Table

| Feature | Superset | Codemux | Effort |
|---|---|---|---|
| `gh` CLI wrapper infrastructure | Yes | Yes (shared) | — |
| Auth detection and graceful degradation | Yes | Yes (shared) | — |
| GitHub repo detection | Yes | Yes (shared) | — |
| Fetch open issues (`gh issue list`) | Yes | No | Small |
| Fetch full issue content (`gh issue view`) | Yes | No | Small |
| Issue data types (Rust struct + TS type) | Yes | No | Small |
| Tauri commands for issue operations | Yes (tRPC) | No | Small |
| Issue picker popover (cmdk/Command) | Yes | No | Medium |
| Fuzzy search (by number + title) | Yes (Fuse.js) | No | Small |
| Direct URL pasting in search | Yes | No | Small |
| State icon per issue (open/closed) | Yes | No | Small |
| Multiple issues per workspace | Yes | No | Medium |
| Issue pill with unlink button | Yes | No | Small |
| Issue body as agent context (markdown) | Yes | No | Medium |
| Auto-name branch from issue | No | No | Medium |
| Persist issue link in workspace state | No | No | Medium |
| Show linked issue on workspace sidebar | No | No | Small |
| Auto-refresh issue state | No (PR only) | No | Medium |
| Close issue on PR merge | No | No | Medium |
| Show issue comments/activity | No | No | Large |
| Filter by label/assignee | No | No | Medium |
| Issue creation from within app | No | No | Large |
| Control socket issue commands | N/A | No | Small (Phase 1) |
| CLI `codemux issue list/view/link` | N/A | No | Small (Phase 1) |
| Cross-repo issue linking (URL-based) | Yes | No | Small |

---

## Section 4: Implementation Plan

Ordered by impact — the issue picker in workspace creation ships first.

### Phase 1: Backend Issue Foundation + CLI/Socket Surface (Small)

**Goal**: Add issue types, fetching commands, Tauri IPC, control socket commands, and CLI subcommands. Agents need programmatic access from day one.

**Rust backend** (`src-tauri/src/github.rs`):
1. Add `GithubIssue` struct:
   ```
   GithubIssue { number, title, url, state, labels: Vec<String> }
   ```
2. Add `GithubIssueDetail` struct (for full view):
   ```
   GithubIssueDetail { number, title, body, url, state, author, created_at, updated_at, labels }
   ```
3. Add `list_github_issues(repo_path) -> Vec<GithubIssue>`:
   - Run `gh issue list --state open --limit 30 --json number,title,url,state,labels`
   - Parse with serde, normalize state strings
   - 10-second timeout via `std::time::Duration`
4. Add `get_github_issue(repo_path, number) -> GithubIssueDetail`:
   - Run `gh issue view <number> --json number,title,body,url,state,author,createdAt,updatedAt,labels`
   - Truncate body to 50KB

**Tauri commands** (`src-tauri/src/commands/github.rs`):
5. Add `list_github_issues(path: String) -> Result<Vec<GithubIssue>, String>`
6. Add `get_github_issue(path: String, number: u32) -> Result<GithubIssueDetail, String>`

**Frontend types** (`src/tauri/commands.ts` + `src/tauri/types.ts`):
7. Add TypeScript types matching Rust structs
8. Add command wrappers: `listGithubIssues(path)`, `getGithubIssue(path, number)`

**Control socket** (`src-tauri/src/control.rs`):
9. Add `list_github_issues` command — accepts `{ repo_path: String }`, returns `Vec<GithubIssue>` as JSON
10. Add `get_github_issue` command — accepts `{ repo_path: String, number: u32 }`, returns `GithubIssueDetail` as JSON
11. Add `link_workspace_issue` command — accepts `{ workspace_id: String, number: u32, title: String, url: String }`, sets issue fields on workspace state (depends on Phase 4 state fields, but define the command shape now)

**CLI subcommands** (`src-tauri/src/cli.rs`):
12. Add `Issue` variant to `CommandSet` enum:
    ```
    Issue { command: IssueCommand }
    ```
    where `IssueCommand` is:
    ```
    enum IssueCommand {
        List,                          // codemux issue list
        View { number: u32 },          // codemux issue view 92
        Link { number: u32 },          // codemux issue link 92 (links to active workspace)
    }
    ```
13. `codemux issue list` — sends `list_github_issues` to control socket using active workspace's project root, prints table: `#number  title  state`
14. `codemux issue view 92` — sends `get_github_issue`, prints issue body to stdout (useful for piping into agents)
15. `codemux issue link 92` — sends `link_workspace_issue` for active workspace (deferred until Phase 4 state fields exist, but define the CLI shape now)

### Phase 2: Issue Picker UI (Medium)

**Goal**: Add issue search/link popover to workspace creation dialog.

**New component** (`src/components/overlays/github-issue-picker.tsx`):
1. Build using shadcn `Command` + `Popover` (consistent with existing command palette pattern)
2. Fetch issues on popover open (not on dialog mount)
3. Client-side fuzzy search with Fuse.js (weighted: number x3, title x2)
4. Support direct issue URL pasting
5. Each row: state icon (colored dot), `#number` mono, truncated title
6. Loading/empty/error states
7. "Link" action on select → callback to parent

**Dialog integration** (`src/components/overlays/new-workspace-dialog.tsx`):
8. Add `linkedIssues` state array to dialog
9. Add "Link GitHub Issue" button (only visible when `ghAvailable && isGithubRepo`)
10. Position below existing PR linking dropdown
11. Show linked issue pills below prompt textarea (similar to attachment pills)
12. Each pill: issue title (truncated), `#number`, state icon, X to unlink
13. When first issue is linked and branch name is empty, auto-suggest branch name: `{number}-{slugified-title}` (e.g., `92-backend-endpoints`)
14. User can accept or edit the suggested name

### Phase 3: Issue Context Injection (Medium)

**Goal**: Feed linked issue body into agent prompt/context at workspace creation. Two distinct injection paths exist for terminal workspaces vs OpenFlow runs.

**Backend** (`src-tauri/src/commands/workspace.rs`):
1. Extend `create_worktree_workspace` to accept `linked_issue: Option<LinkedIssueRef>` where `LinkedIssueRef = { number: u32, title: String, url: String }`
2. At creation time, fetch full issue body via `get_github_issue()` — 10s timeout, 50KB truncation
3. Handle fetch failure gracefully: log warning, proceed without issue context (issue context is supplementary, not blocking)

**(a) Terminal workspace injection** — file-based approach:
4. Write issue body to `.codemux/issue-context.md` in the worktree root:
   ```markdown
   # GitHub Issue #92: Backend Endpoints
   Source: https://github.com/org/repo/issues/92

   {issue body}
   ```
5. Modify the `initial_prompt` to reference it:
   ```
   Context: GitHub Issue #92 — see .codemux/issue-context.md for full details.

   {user's original prompt}
   ```
6. This works with both injection paths in `prepare_agent_command()` (`branch_name.rs:163-189`):
   - **Claude/Codex presets**: prompt embedded as quoted argument — the file reference is short and shell-safe
   - **Other agents** (Gemini, OpenCode): prompt injected via PTY through `write_command_when_ready()` (`presets.rs:403-511`) — same, the file reference is a short string

   Why file-based: issue bodies can be 50KB. Shell-escaping that into a command argument (`shell_escape_for_double_quotes()`) is fragile and can exceed shell argument limits. The file is always available in the worktree for agents to read on demand.

**(b) OpenFlow run injection** — inline in goal:
7. For OpenFlow runs, the goal flows through `OpenFlowCreateRunRequest.goal` → written to `goal.txt` → injected into the orchestrator's initial message (`openflow.rs:532-548`). Append issue context directly to the goal text:
   ```
   {user's goal}

   ---
   LINKED GITHUB ISSUE #92: Backend Endpoints
   Source: https://github.com/org/repo/issues/92

   {issue body, truncated to 50KB}
   ```
   Why inline: the orchestrator expects a self-contained goal blob. It gets passed to spawned agents via the communication protocol, so agents automatically receive the issue context without needing file access.

**Frontend** (`src/components/overlays/new-workspace-dialog.tsx`):
8. Pass `linkedIssue` in `createWorktreeWorkspace()` call
9. No frontend change needed for the injection mechanism — backend handles it based on workspace type

### Phase 4: Workspace State Persistence (Medium)

**Goal**: Store linked issue reference in workspace metadata so it persists and displays.

**Rust state** (`src-tauri/src/state/`):
1. Add fields to workspace state: `issue_number: Option<u32>`, `issue_title: Option<String>`, `issue_url: Option<String>`, `issue_state: Option<String>`
2. Add `set_workspace_issue_info()` method (parallel to `update_workspace_pr_info()`)
3. Call from `create_worktree_workspace` after workspace creation
4. Include in `WorkspaceSnapshot` serialization

**Frontend** (`src/tauri/types.ts`):
5. Add issue fields to `WorkspaceSnapshot` TypeScript type

**Sidebar enrichment** (`src/components/workspace/workspace-row.tsx` or equivalent):
6. Show linked issue badge on workspace row (e.g., `#92` with state icon)
7. Click to open issue URL in browser pane

### Phase 5: Auto-Branch Naming (Medium)

**Goal**: Smart branch name generation from linked issue.

**Logic** (frontend, in workspace creation dialog):
1. When first issue is linked and branch name field is empty:
   - Generate: `{number}-{slugified-title-max-50-chars}`
   - Slugify: lowercase, replace non-alphanumeric with hyphens, collapse multiple hyphens, trim
   - Example: Issue #92 "Add backend API endpoints for user management" → `92-add-backend-api-endpoints-for-user-management`
2. Set as branch name input value (user can edit)
3. If user has already typed a branch name, do NOT overwrite — just show suggestion as placeholder

### Phase 6: Issue State Refresh (Medium)

**Goal**: Keep linked issue state current in workspace metadata.

**Approach: Refresh-on-focus + manual refresh** (not polling).

Rationale for NOT polling:
- The existing PR refresh loop (`lib.rs:300-331`) polls every 5s but only for the *active* workspace. Issue state changes far less frequently than PR state (no checks, reviews, or deployments ticking).
- With many workspaces linked to issues, polling all of them would spam `gh` calls. The `gh` CLI has no rate limit handling — GitHub API rate limit is 5000/hr authenticated, and each workspace would consume one call per poll interval.
- Polling is overkill for a state that changes maybe once per day.

**Backend** (`src-tauri/src/lib.rs`):
1. In the existing active-workspace refresh loop, add: if active workspace has `issue_number`, fetch current state via `gh issue view <number> --json state` alongside the PR fetch. This piggybacks on the existing loop — one extra `gh` call only for the workspace the user is looking at.
2. Add `refresh_workspace_issue` Tauri command — fetches issue state on demand, called when:
   - User switches to a workspace with a linked issue (workspace-focus event)
   - User clicks a refresh button on the issue badge

**Frontend**:
3. On workspace switch (`useEffect` on active workspace change), if workspace has `issue_number`, call `refresh_workspace_issue`
4. Add refresh icon on the issue badge in workspace row — click triggers manual refresh
5. No background timer on the frontend side

### Future Phases (Not in Initial Scope)

These are deferred but documented for completeness:

- **Issue comments panel**: Parallel to PR reviews panel — show issue comments, post new comments
- **Close issue on PR merge**: Detect when linked PR merges, offer to close linked issue
- **Issue creation**: Create new issues from within Codemux (title + body + labels)
- **Label/assignee filtering**: Extend issue picker with filter dropdowns
- **MCP server tools**: Expose issue operations as MCP tools (control socket + CLI surface ships in Phase 1)

---

## Section 5: Architecture Decisions

### Should we use `gh issue list --json` or GitHub API directly?

**Decision: Use `gh` CLI.**

Rationale:
- Codemux already uses `gh` for all PR operations — consistent pattern
- Avoids managing GitHub OAuth tokens separately (reuses `gh auth login`)
- `gh` handles pagination, rate limiting, and auth refresh internally
- Graceful degradation already works (check `gh_available()` + `check_gh_status()`)
- If `gh` ever becomes a bottleneck, we can swap to direct API calls later without UI changes

### Should issue data be cached? Where?

**Decision: Minimal caching, frontend-side only.**

Rationale:
- Issue lists are small (30 items max) and fast to fetch
- Cache in React component state or a lightweight zustand slice — refetch when popover opens
- No backend cache needed — `gh` calls are fast enough for interactive use
- PR integration already works without backend caching (5s polling loop)
- Cached issue state on the workspace (issue_number, issue_state) is persisted in workspace state, but that's metadata, not a query cache

### Should linking be required or optional?

**Decision: Optional.**

Rationale:
- Many workspaces are exploratory, quick fixes, or not tied to a specific issue
- Existing workflow (type branch name, pick preset, go) should not be slowed down
- Issue picker is additive — a button that opens a popover, not a required step
- Superset also makes it optional

### How should branch naming work?

**Decision: Auto-suggest from issue, user can edit.**

Pattern: `{issue_number}-{slugified-title}` (e.g., `92-backend-endpoints`)

Rationale:
- Including the issue number makes branches self-documenting and enables GitHub's auto-linking (`Fixes #92`)
- Slugified title adds human readability
- User retains full control — suggestion appears only when branch field is empty
- Superset does NOT do this (uses random words) — this is an improvement over their approach
- No prefix like `feature/` or `fix/` — keep it simple, let users add their own convention

### Should we support issue creation from within Codemux?

**Decision: Defer to future phase.**

Rationale:
- Issue creation is a nice-to-have, not part of the core "link issue to workspace" flow
- The primary use case is linking existing issues, not creating new ones
- Adding `gh issue create` is straightforward when needed but adds UI complexity (labels, assignees, templates)
- Ship the linker first, validate demand, then add creation

### Should we support multiple linked issues per workspace?

**Decision: Start with single issue, design for multiple.**

Rationale:
- Most workspaces address one issue — single link covers 90% of cases
- State fields (`issue_number`, `issue_title`, etc.) are simpler as scalars
- The picker UI and context injection can support selecting one issue
- If demand exists, migrate to `Vec<LinkedIssue>` later (the picker already supports it naturally)
- Superset supports multiple but rarely uses more than one in practice

---

## Likely Touch Points

- `src-tauri/src/github.rs` — new issue types and fetch functions
- `src-tauri/src/commands/github.rs` — new Tauri commands
- `src-tauri/src/commands/workspace.rs` — extend creation to accept issue ref, write `.codemux/issue-context.md`
- `src-tauri/src/state/state_impl.rs` — add issue fields to workspace state
- `src-tauri/src/lib.rs` — extend active-workspace refresh to include issue state
- `src-tauri/src/control.rs` — add `list_github_issues`, `get_github_issue`, `link_workspace_issue` socket commands
- `src-tauri/src/cli.rs` — add `Issue` variant to `CommandSet`, `IssueCommand` enum
- `src-tauri/src/openflow/mod.rs` — accept issue context in `OpenFlowCreateRunRequest`
- `src/tauri/commands.ts` — new command wrappers
- `src/tauri/types.ts` — new TypeScript types
- `src/components/overlays/new-workspace-dialog.tsx` — integrate issue picker
- `src/components/overlays/github-issue-picker.tsx` — new component
- `src/components/workspace/workspace-row.tsx` — issue badge display

## Open Questions

- Should the issue picker also show closed/all issues, or only open? (Start with open only, add a toggle later)
- Should issue linking work for non-worktree workspaces? (Probably not — issues map to branches)
- Should we show issue body preview on hover in the picker? (Nice-to-have, defer)
- How should we handle repos with 1000+ open issues? (30-item limit with search is likely sufficient; revisit if users complain)

## Already Landed

- Full `gh` CLI infrastructure (run_gh, run_gh_json, run_gh_optional)
- GhStatus auth detection
- PR integration (full lifecycle)
- Workspace creation with worktree support
- Background PR refresh loop
- Workspace state persistence with PR fields
- **Phase 1 complete**: GitHub issue types (`GitHubIssue`, `LinkedIssue`, `IssueState`), issue fetching (`list_github_issues`, `get_github_issue`), workspace issue linking/unlinking/refresh, auto-branch name suggestion (`feature/{number}-{slug}`), Tauri commands (8 total including path-based variants), control socket commands (3), CLI subcommands (`codemux issue list/view/link`), frontend TypeScript types and command wrappers, background refresh for linked issue state, 10s timeout on `gh` commands via `run_gh_timed`
- **Phase 2 complete**: Issue picker in workspace creation dialog (searchable, fuzzy filter, debounced server search, keyboard navigation, skeleton loading), toolbar icon buttons (attach, PR, issue, submit), linked issue chip with auto-branch naming and auto-fill tracking (unlink/switch clears correctly), linked issue badge on sidebar workspace rows, post-creation linking via `linkWorkspaceIssue`
- **Phase 3 complete**: Issue detail popover on sidebar (click `#number` to view full issue — title, labels, assignees, scrollable body, "Open on GitHub" via Tauri openUrl), prompt auto-injection (linked issue context prepended to agent prompt at workspace creation, body truncated at 10K chars, graceful degradation on fetch failure), `buildPromptWithIssueContext` pure function

## Notes

- Superset treats issues as ephemeral creation-time context (no persistence). Codemux goes further by persisting the link in workspace state, showing it on the sidebar, and providing a detail popover.
- The existing PR integration was the architectural template — issue support follows the same patterns (Rust types → Tauri commands → React components → workspace state).
- OpenFlow issue injection is deferred — the OpenFlow dialog creates its own workspace and has no linked issue concept yet.
