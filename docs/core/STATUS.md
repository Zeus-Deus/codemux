# Codemux Status

- Purpose: Canonical reality snapshot for the repo.
- Audience: Anyone deciding what is actually true today.
- Authority: Current implementation truth.
- Update when: Behavior, constraints, or known gaps change.
- Read next: `docs/core/PLAN.md`, `docs/core/TESTING.md`

## Current Headline

This section covers what is unreleased on `main` plus the two most recent releases
(`v0.17.1` and `v0.17.0`). The version in
`package.json`/`Cargo.toml`/`tauri.conf.json` is **`v0.17.1`** (released 2026-08-07);
everything listed under "Unreleased after `v0.17.1`" below landed after that tag and has
not shipped in a published build. Older per-release implementation notes were moved to
`docs/archive/release-notes-v0.16.md` (`v0.16.0`),
`docs/archive/release-notes-v0.14-v0.15.md` (`v0.14.0`–`v0.15.6`) and
`docs/archive/release-notes-v0.6-v0.13.md` (`v0.6.1`–`v0.13.2`);
`docs/core/PLAN.md` § "Recently Completed" carries the release-ordered summary, and the
`docs/features/*` docs carry the current behavior of every subsystem named below.

Codemux is past Linux MVP and shipping cross-platform binaries. The workspace shell, terminal management, git integration, presets, settings sync, and most ADE features are real and daily-drivable on both Linux and Windows. The latest released version is **`v0.17.1` (2026-08-07)**, a sidebar-and-settlement release: a fifth `Monitoring` agent status for background watch loops, durable workspace pinning, a run that can no longer stay stuck on "Working", side-branch PR badges, the dimmed Settled shelf, and AppImage child-process environment hygiene. `v0.17.0` (2026-08-05) was the app-chrome and chat-navigation release: the GUI title bar became frameless floating islands, the full-width workspace context bar was deleted, the chat transcript gained an explicit new-turn scroll contract, cross-provider skills became provider-correct, text-selection colors were standardized onto two tokens, and the legacy standalone orchestration runtime was deleted outright. Release notes for `v0.16.0` and earlier live in `docs/archive/release-notes-v0.16.md`, `docs/archive/release-notes-v0.14-v0.15.md`, and `docs/archive/release-notes-v0.6-v0.13.md`.

Unreleased after `v0.17.1` — the current Canvas-6 work, the Settings → Usage
dashboard, plus five merged PRs (#257–#261), grouped by subsystem below.
Everything in this block is current checkout behavior and has not shipped in a
published build:

- **Settings → Usage: provider-wide local history for Claude Code, Codex, and OpenCode.** Provider-owned durable history is now the single accounting source regardless of launcher: Claude and Codex JSONL transcripts plus OpenCode's read-only SQLite message history (with legacy JSON fallback). The ledger is a rebuildable materialized cache keyed by provider-native record ids; growing responses upsert instead of freezing partial counts, resumed/forked records deduplicate, and importer v3 clears the legacy runtime/import hybrid so Codemux-launched work cannot double-count. Subagent attribution and the four-way non-overlapping token split remain. The page reports one clearly labelled API/list-price-equivalent estimate—never guessed “billed” or “plan-covered” spend—plus tokens, sessions, model breakdown, confidence, CSV, and an honest this-machine-only source note. Open/manual/30-second refreshes all scan provider history first. See `docs/features/usage-dashboard.md`.

- **Settings → Usage: live quota is independent from historical cost.** `ProviderRuntimeEvent::PlanUsageUpdated` still carries fresh account-level quota windows and provider plan labels into an in-memory newest-snapshot store. It is never persisted or used to classify historical dollars. Claude/Codex lanes show up to two provider-reported windows when available; a provider with no current reading shows no meter. See `docs/features/usage-dashboard.md`.

- **The agent activity indicator is contextual** (PR #260, refined after `v0.18.0`). The configurable working-indicator glyph (braille / ring / blink / sweep / typing) and its six color swatches are **deleted** — `WorkingIndicator`, `AsciiSpinner`, `sidebar.working_indicator`, and `sidebar.working_indicator_color` are all gone. Agent-local working/running indicators render `AgentOrb` (`src/components/ui/agent-orb.tsx`) over the MIT `thinking-orbs` library: the in-flight turn (the Activity block header, with `StreamingMarker` covering the dead time either side), subagent rows, the docked composer strip, and workflow/orchestration run headers. The expanded sidebar workspace card instead uses a static amber `CircleDotDashed`: workspace navigation knows lifecycle but not the thread's current action, and the collapsed rail already uses a static status dot. Orbs remain monochrome — the library inks from `theme="auto"` off the root `dark` class, so no call site passes a color and red stays reserved for state needing a human. Sizes are 20 (inline) and 64 only. A single shared helper, `src/lib/orb-state.ts`, maps the agent's current tool to a state (searching / composing / working / solving / connecting / weaving / listening / breathing, neutral `working` fallback), fed by `src/lib/agent-chat/orb-activity.ts` from tool-call name + input + status the reducer already stamps — no new backend plumbing. Shell tools read their command string, so `git push` reads as connecting and `cargo test` as working. Design rule enforced by tests: **one orb per live thing** — aggregate headers and the composer strip stay neutral, each running row owns its own, and finished rows revert to the flat check. Settings → Appearance → Agents lost both pickers and gained a **Match the orb to the activity** toggle (`agents.orb_match_activity`, default on); off pins every rendered orb to `working`. Migration is a **drop, not a map**: the machine-local settings table is a free-form `Record<string, string>` with no Rust-side schema, so old keys are simply never read and never written back. See `docs/features/agent-chat.md` § "Agent activity orb", `docs/features/sidebar.md`, `docs/features/settings.md`.

- **Subagent work is now one line per uninterrupted stretch** (Canvas-6 Turn 2, superseding the Canvas-3 rail). Canonical per-turn `subagent_run` events are unchanged, but `transcript-slots.ts` presentation-merges adjacent runs across invisible turn-end markers into one `subagent_stretch`; visible prose/tools split it. The transcript spends one labelled 32px row for 1 or 40 agents (`Ran N subagents` · preview/real rollup · `View ›`) and never expands child rows inline. View opens and marks the right-panel **watch surface**, which owns aggregate progress, live activity-matched cards, finished rows, and `Open thread` drill-in. The composer strip now hides while its matching work-log row is visible and returns only as the off-screen tether. Browser-open activity uses the same compact row grammar while the context-row browser indicator remains live. A lone successful `Read`/`Grep`/`Glob` is now silent (bursts still roll into one Activity summary; running/error/approval/image calls remain). Persistence, provider mapping, and drill-in routing remain canonical-run-based; zero-size run-id anchors keep every pre-existing jump/highlight target valid after a merge. See `docs/features/agent-chat.md` § "Subagent view (cross-provider)", § "Composer running strip", and `docs/features/browser.md` § "Background browser in GUI mode".

- **The right panel's chrome moved into the titlebar band, and the empty panel became a picker** (PR #260, Canvas-3 design handoff, change 2 of 3). The panel used to reserve a blank `mt-10` strip for the floating titlebar and start its 36px tab row underneath it, so an open panel wore an empty ~40px header and read as a pane inside a pane; worse, the right-panel toggle rode in the titlebar's action island, whose right edge tracked `rightPanelWidth + 8`, so opening the panel teleported that button from the window's top-right corner to the panel's left edge. The tab row now **is** the band — `h-10`, flush with the window's top edge, body starting exactly at its bottom edge — and the panel-level controls moved to a **fixed top-right cluster** (`⤢` full expand + panel toggle) that sits at the same corner in every state. `src/lib/titlebar-geometry.ts` is the one place those numbers live, since the overlay and the panel's row are in different React trees: the cluster sits `104px` from the right edge (`6px` on the web client), and both the workspace band's closed-state right edge and the panel row's right padding derive from it (`166px`). Window dragging survives: the overlay's drag layer now stops at the panel's left edge instead of spanning the window (it would otherwise cover every tab), and the flex gap after the panel's tabs carries `data-tauri-drag-region` itself — desktop-only in both cases, as before. **Full expand** hands the panel the whole content row (workspace column to `w-0 flex-none`, still mounted; workspace band hidden) and restores the *exact* previous width, because maximizing writes no width at all — the column just drops its inline width and swaps `shrink-0` for `flex-1`. The flag is runtime-only and cleared on collapse. Closing the last tab now lands on an **"Open a surface" picker** — a card grid of Terminal / Files / Changes / Diff / Review / Browser with one-line descriptions — instead of collapsing the panel; the grid renders the same `SurfaceAction[]` the `+` menu does, so the two can't drift, and collapsing is still one click on the panel toggle. `rightPanelTabs[ws]` gained a third state, `RIGHT_PANEL_EMPTY`. Legacy `h-9` chrome is untouched: no cluster there, so the panel's row keeps its own 36px height and its expand/close pair. See `docs/features/gui-chrome.md` § "Band geometry", `docs/features/agent-chat.md` § "Right panel — the pane deck".

- **The browser lives in the right panel now, and the panel got wide** (PR #260). The right-panel deck's `+` menu no longer routes **Browser** out to `createBrowserPane` (a main-area split) — `browser` is a real registry pane, and opening it **docks** the workspace's existing `AgentBrowserSession` into the deck. No browser session state is forked: `BrowserPane` is a canvas fed by a WebSocket screencast (position-independent — the peek overlay already mounted one outside the pane tree), and the deck mounts it against the session's `cli_session_name`, the same daemon key every `codemux browser` CLI call and MCP tool resolves to. `AgentBrowserSession` gained `right_panel_docked`, mutually exclusive with `pane_id` by construction, and `is_surfaced()` (`pane_id.is_some() || right_panel_docked`) replaced the bare `pane_id` test in `should_create_browser_pane` and `release_detached_agent_browser` — so an agent driving a docked browser doesn't split a second surface for it, and a run ending doesn't mark a browser the user is watching inactive. Two new commands, `dock_browser_in_right_panel` / `undock_browser_from_right_panel`, mirror the `browser_automation` handler's resolve → allocate port → write back sequence; docking **adopts** (closes) a main-area pane already holding the session rather than mirroring it, and the deck yields its tab if the session is later re-attached to a pane-tree node. The peek overlay's promote button became **"Open in side panel"** and docks instead of splitting, so a browser has exactly one persistent home. The chip / terminal-header indicator / peek overlay now share one `selectBackgroundBrowserSession` predicate instead of three copies of it. Separately, the panel's width cap moved from a flat 500px to a layout-aware rule in `@/lib/right-panel-width`: at most **75% of the row it shares with the workspace content** (measured, because the left sidebar is separately resizable and sits outside that row), never leaving the chat side below 240px. The *stored* width is what the user asked for and is only sanity-bounded, with the layout clamp applied at render time, so a panel sized on an external monitor survives a session on a laptop screen. See `docs/features/browser.md` § "The browser in the right-panel deck", `docs/features/agent-chat.md` § "Right panel — the pane deck".
- **Ultracode reasoning level for Claude + Codex effort-picker polish**
  (PR #261). The reasoning picker now offers
  **Ultracode** on every xhigh-capable Claude model: `ensure_ultracode_effort`
  (`agent_provider/claude/capabilities.rs`) appends the level at all three
  capability-producer paths (maintained fallback, sidecar-harvest merge — so
  live-harvested models like Opus 5 get it — and API harvest). Because older
  headless CLIs reject `--effort ultracode`, the level is normalized at the
  launch boundaries: the sidecar's `buildQueryOptions` sends SDK
  `effort: "xhigh"` + `settings: {ultracode: true}` (standing multi-agent
  workflow orchestration, pairing with the existing Workflow orchestration
  card), and the terminal-preset splice (`agent_capability.rs`) emits
  `--effort xhigh --settings '{"ultracode":true}'` idempotently — unless the
  preset already carries a `--settings` of its own, which the CLI resolves
  last-wins rather than merging. Then the splice folds `"ultracode": true`
  into an inline-JSON value (a single merged flag), or falls back to
  `--effort ultracode` for a file-path value, so a user's hooks and
  permissions are never silently dropped. On the Codex
  side, the catalog-advertised `max`/`ultra` levels (already sent verbatim on
  `turn/start`, which is protocol-correct — `ultra` also enables the
  provider's proactive multi-agent mode) now render Title-Case labels, and a
  new additive `ChatModelInfo.effort_descriptions` map carries the Codex
  catalog's per-effort descriptions into the picker's description line, with
  built-in fallbacks for `ultra`/`ultracode` in `ReasoningPicker.tsx`. See
  `docs/features/agent-chat.md` § "Reasoning picker (effort)".

- **Popup menus restyled onto the command-palette surface** (PR #258). The workspace
  right-click menu, the footer gear menu and the projects `+` menu now share
  one chrome (`src/components/ui/menu-chrome.tsx` + `.cm-menu-surface` in
  `globals.css`): 13px radius, hairline border, one elevation, 32px rows with
  a 14px icon on every row, and right-aligned mono keycaps — resolved through
  `keybind-registry` where a binding exists, omitted where none does (no new
  global bindings were registered, and a keycap is never shown for a combo the
  registry hands to a different action). The item lists are unchanged.
  New presentation: an identity header on the workspace menu (avatar,
  workspace, project, `+A −D`), named **Workspace / Actions / Organize** groups
  instead of bare dividers, a red-tinted destructive tail, the project colour
  palette as a 7-across swatch grid, a two-line device submenu that always
  exists (with "No other devices signed in." when it is empty), two-line rows
  in the `+` menu, and a version/update footer strip in the gear menu fed by
  the new `src/stores/update-status-store.ts` mirror — so the menu reads the
  one `useUpdateChecker` that `UpdateToast` mounts rather than starting a
  second poll. The shadcn dropdown/context-menu primitives adopted the surface
  globally (submenus included, which previously diverged); the row rhythm is
  opt-in, so the ~20 other menus on those primitives keep their dense lists.
  See `docs/features/sidebar.md` and `docs/features/project-avatars.md`.

- **Text selection is restricted to content, not UI chrome** (PR #259, plus the follow-up opt-in pass). Codemux adopted the desktop-app convention that drag-selection only ever grabs real content: the document opts out once at the root (`body { user-select: none; cursor: default }` in `src/globals.css` `@layer base`, with the `-webkit-` prefix WebKitGTK requires) and content opts back in through two tiers of the same base layer — automatic element/root selectors (`input`, `textarea`, `[contenteditable]`, `pre`, `code`, `kbd`, `samp`, `.cm-content`, `.chat-markdown`, `.markdown-rendered`, sonner toast text), which re-enable a whole subtree because `user-select` inherits, and the `select-text` utility for one-off prose/mono blocks with no shared root. A base-layer companion rule restores the I-beam on re-enabled text, and a base-layer `a { cursor: pointer }` keeps links from reading as dead text now that `cursor` inherits the body's `default`. Chrome nested inside a content root opts out again with `select-none`, so precedence works in both directions. The follow-up pass covered the surfaces the first sweep missed and that users actually copy: the chat diff-card body, inline error/notice text, Read/Grep/Edit file-path headers, the Grep match list, the WebFetch title/prompt, and the selected host's `ssh_target`. Frontend + docs only. See `docs/reference/DESIGN-SYSTEM.md` § "Selectability: chrome is not selectable, content is".

- **Multi-provider source control — GitLab joins GitHub** (PR #257, the
  source-control provider seam). The hosting integration was GitHub-only in two places at once:
  every call shelled out to `gh`, and the gate for "may we show PR UI here?" was
  a substring match on `github.com` in `git remote -v`. Both halves are now a
  seam under `src-tauri/src/git_provider/`. **Detection** is offline — `git
  remote -v` plus the branch's upstream, classified by *hostname* into a
  `ProviderKind` (`github` / `gitlab` / `bitbucket` / `azure_devops` /
  `unknown`), with a synced `source_control.custom_hosts` mapping outranking
  every built-in heuristic so a self-hosted instance on a neutral domain
  (`git.acme.internal`) can be named by the user. The multi-remote policy is
  stated once: a classifiable remote always beats an unclassifiable one (the old
  gate matched *any* remote and must keep doing so), then upstream → `origin` →
  first listed. Results are cached for 60s **success-only**, so an `Unknown` is
  re-probed every call and a `git remote add origin …` or a fresh host mapping
  takes effect on the next render rather than a minute later; a settings write
  that moves the mapping clears the cache outright. **`SourceControlProvider`**
  is exactly the method set the review panel, composer pickers, worktree path and
  both pollers already called on `crate::github`, reusing the same structs so a
  second product populates the same shapes rather than inventing new ones; the
  registry resolves a checkout to an adapter with a deliberate split — *strict*
  for gates and pollers (`repo_has_supported_provider`, replacing
  `is_github_repo`), *advisory* for the GitHub-named command surface, so a host
  Codemux cannot classify still reaches `gh` and self-hosted GitHub Enterprise
  keeps working. Unserved products get a null object, not an `Option`, so no
  caller branches on "is there a provider" and every failure is one sanitized
  sentence. **The GitHub adapter is logic-free delegation** — routing a call site
  through the trait cannot change what GitHub users observe. **The GitLab
  adapter** drives `glab api` (the documented REST payload, not glab's own Go
  struct, and the only way to reach discussions/pipelines/per-file diffs),
  host-scoped by `--hostname` because an unscoped `glab auth status` ANDs across
  every configured instance; it normalizes merge requests onto `PullRequestInfo`
  by `iid`, maps pipeline jobs (or the commit statuses standing in for an
  externally reported pipeline) onto the same check buckets, splits discussions
  into conversation threads and diff-anchored inline comments, and answers the
  fork fetch with `refs/merge-requests/<n>/head` against GitHub's
  `pull/<n>/head`. One deadline-bounded subprocess runner (`exec.rs`) now backs
  both CLIs, draining both pipes on their own threads — the previous poll-then-read
  shape deadlocked on any output larger than a pipe buffer and reported a timeout
  that never happened. Both pollers stamp a new snapshot field **`provider_kind`**
  (snapshot-local, serde-defaulted, never synced) and gate per provider *instance*,
  so a signed-out GitHub can no longer stall GitLab workspaces and two self-hosted
  deployments keep independent logins. Two new commands: `discover_source_control`
  (infallible per-product diagnostics — CLI presence, sanitized version line,
  account, declared capabilities; never reads a token) and `check_provider_auth`
  (host-scoped readiness for one checkout, behind a 60s success-only frontend
  cache in `src/lib/provider-auth.ts` shared by the review panel, both composer
  surfaces and the new-workspace preflight, all of which previously asked `gh`'s
  one global question). **Copy is provider-aware** through a single presentation
  map (`src/lib/source-control.ts`): PR/MR nouns, `#`/`!` sigils, CLI names and
  login commands across the review panel, incoming list, Context Row chip,
  sidebar cards/rows/badges, hover card (which gains a **Hosting** row), composer
  attach popup and mention footers, pickers, and the new-workspace dialog — with
  an absent `provider_kind` resolving to GitHub so every existing GitHub surface
  renders byte-identical strings, and a *present but unrecognised* one falling to
  neutral "change request" wording rather than borrowed GitHub nouns. A new
  **Settings → Source Control** section (`source-control-section.tsx`, plus
  `SubsectionHeader` extracted into `settings-primitives.tsx`) carries the
  diagnostics rows with their one actionable fix line, a Rescan button, and the
  self-hosted host→product editor. Codemux stores no hosting credentials — each
  product is driven through its own CLI's existing login. Bitbucket and Azure
  DevOps are recognised, listed dimmed, and refused with a clear message rather
  than a confusing CLI error; GitLab deliberately serves no deployments and
  refuses `request-changes` rather than silently downgrading it to a comment.
  Verified against a local GitLab by the `#[ignore]`d round trip in
  `src-tauri/tests/gitlab_live.rs`. See
  `docs/features/source-control-providers.md`.

Shipped in `v0.17.1` (2026-08-07) — nine merged PRs (#248–#256) plus the
directly-committed side-branch PR badge fallback, grouped by subsystem below:

- **AskUserQuestion panel rebuilt on the shadcn Questionnaire component** (PR #252). `ComposerPendingInputPanel`'s hand-rolled option rows, paging chevrons, and hidden `sr-only` inputs are replaced by the newly released shadcn **Questionnaire** primitives (`src/components/ui/questionnaire.tsx` over the new `@shadcn/react` package, installed for the repo's `radix-nova` style): real fieldset/legend semantics per question, radio/checkbox indicator cards with automatically mapped 1–9 shortcut chips, focus-visible rings the old panel lacked, and Previous/Next/Submit navigation. The externally observable contract is unchanged — same `AskUserQuestionOutput` shape (answers keyed by question text, `", "`-joined multiSelect with free text appended, raw `questions` echoed), same composer-docked card on the chat-column rails, same global document-level keyboard layer for focus-outside-the-form digits/arrows/Enter (now guarded against double-handling with the primitive's in-form handler), same `option.preview` HoverCards and test ids. `npm run dev` + `?askq=1` seeds a pending two-question request for browser QA; the mock's `agent_chat_respond_to_request` now resolves it so answering settles the panel into the reply bubble. See `docs/features/agent-chat.md` § "AskUserQuestion panel".

- **A finished run can no longer stay stuck on "Working"** (PR #254).
  A run that launched a background shell command (`Bash { run_in_background:
  true }`) kept the sidebar spinner, the background-browser `LIVE` chip, the
  docked subagent activity bar, and the composer's `Tasks N/M` spinner alive
  indefinitely after the turn finished. Claude emits the same `system.task_*`
  family for a background tool run as for a real subagent, so the pane-status
  tracker put a dev server — which never exits, and therefore never sends the
  terminal `task_notification` — into the per-thread running-set;
  `TurnCompleted` then deferred `Review` forever, and because
  `release_detached_agent_browser` only fires on a settled status, the browser
  chip never cleared either. Three-part fix. **(1) Root cause**:
  `SubagentSnapshot` gained an additive `background_task: bool`
  (`#[serde(default)]`) stamped by the Claude adapter from the discriminator
  it already had — only a real `Agent`/`Task` launch is registered as a
  top-level launch in `SubagentDemux` — and `map_event_to_pane_status` never
  tracks a flagged row, returns `None` for it even when a `Review` is owed
  (so a progress tick can't resurrect `Working` after the run settled), and
  defensively evicts an id an unflagged snapshot inserted earlier. Real async
  `Task` launches keep their deliberate deferred-`Review` flow. **(2)
  Backstop**: the 30s stall-watchdog sweep gained a second pass that
  force-settles an owed `Review` gone silent for the existing 600s
  `STALL_THRESHOLD`, routed through `apply_pane_status` — the helper extracted
  out of `publish_pane_status` — so the `Review`→`Idle` downgrade and the
  stamped emit are shared rather than copied. Any tick from a real tracked
  entry re-arms the clock (a flagged background tick deliberately does not), so
  the threshold means "every remaining blocker has been silent for ten
  minutes". Silence still cannot prove death — one long `cargo test` inside a
  live subagent looks identical — so the forced path is deliberately
  non-destructive: it passes `SettleOrigin::ForcedBackstop`, which **withholds
  the detached-browser release** (a premature dot is repainted by the next real
  event; a browser torn down under a live subagent is not recoverable), and it
  **tombstones** the tracker entry (`forced_settled`) instead of removing it,
  so a late real completion drains normally, publishes nothing, and leaves no
  orphaned entry rather than recreating an uncollectable husk. It can't cut a
  live turn short — `review_pending` is only ever set by `TurnCompleted`.
  **(3) Frontend**: live affordances now respect the end of
  the run. Background-task rows drop out of `runningSubagentEntries` /
  `countRunningSubagents` once the thread stops streaming (shared
  `isLiveActivity` predicate; the flag merges stickily), and the Tasks chip's
  spinner and the Tasks tab's blinking dot are gated on `streaming` rather
  than on an `in_progress` row in the durable snapshot — the chip stays and
  renders its counts statically instead of disappearing. See
  `docs/features/agent-chat.md` §§ "Sidebar status indicators", "Docked live
  activity bar", "Agent Tasks panel" and `docs/features/browser.md`
  § "Run-finished release".

- **Settled rows are dimmed until hovered** (PR #256). The Settled shelf is
  history, so it reads as one grey block at rest rather than a list of
  full-color rows competing with live work: the repo avatar desaturates, the
  title drops to a fainter muted tone, and the PR badge gives up its state
  color (the open/merged/closed hue is deferred, not dropped — it moves to a
  `group-hover/settled:` variant held by the new `prStatusSettledHoverClass`
  beside the existing PR color maps, and the badge's icon inherits
  `text-current` so glyph and number light up together). Hover or keyboard
  focus restores all three at once — nothing is hidden, only ranked. The
  exclusions are the active card's `receded` predicate verbatim, so one
  workspace can't read as "wants you" on a card and "history" on a row: the
  currently-open workspace, any multi-selected row, an **unread** row (`unread`
  is now plumbed into `SettledRow`, which also gives the row's "Mark unread"
  action a visible effect), and a **review**-status row all stay bright — the
  last one matters because the settle safety net deliberately leaves
  finished-and-wants-review work parked. The badge rests at
  `text-muted-foreground/55` rather than the avatar's `/40`, since `#n` is that
  button's only label. Frontend + docs only; no Rust changes. See
  `docs/features/sidebar.md` § "Settle / un-settle".

- **A PR opened from a side branch no longer goes unbadged** (direct commits,
  the sidebar PR fallback). An agent working in a workspace that ran `git checkout -b
  side-branch`, committed, pushed, opened a PR, and checked the worktree back
  left the sidebar with no PR badge at all: association is strictly by
  checked-out branch, and the checked-out branch had none. When the current
  branch resolves to no PR, `github::get_workspace_pr` now falls back to
  branches this worktree checked out recently — `checkout: moving from X to Y`
  records parsed out of the last 50 per-worktree HEAD reflog entries, up to 5
  distinct names newest-first, excluding the current branch, the default
  branch, and detached-HEAD SHAs — and resolves each through the existing
  `select_branch_pr` policy, first hit winning. It badges **open PRs only**
  (unlike the current-branch path, which keeps showing merged/closed state):
  the case it exists for is always an open PR, and admitting history would let
  a `gh pr checkout` of someone else's merged PR donate its badge for the whole
  reflog window. The fallback never runs on the repository default branch, and
  it costs one repo-wide `gh pr list --state open` matched client-side against
  every candidate (not one query per candidate) — memoized per origin URL for
  60s, with the local probes memoized per `(worktree, branch)` for the same
  60s and owner resolution done lazily, so a repeat 5s active-workspace tick
  spawns no subprocess at all. A failed fallback collapses to "no PR" rather
  than `Preserve` — the current branch already answered authoritatively. **The
  association is badge-only.** The workspace snapshot gained
  `pr_head_branch` (snapshot-local, serde-defaulted, never synced) and the
  shared frontend predicate `isPrOnCurrentBranch` compares it to `git_branch`:
  the badge renders identically everywhere, but PR-completion **auto-settle**
  refuses a side-branch PR (that branch merging says nothing about a checkout
  that may still hold uncommitted work — such a workspace still ages out via
  the ordinary inactivity rule), the **"Wrapping up"** demotion refuses it,
  and the **Review tab** stays strictly current-branch so "Create PR" is not
  hidden for the branch the user is actually on. Missing information on either
  side reads as a match — a `null` head branch (an association predating the
  field) *and* a `null` `git_branch` (detached HEAD during a rebase or bisect,
  where the stored head branch outlives the branch name) — so only two known,
  differing names ever un-associate a workspace from its PR. The
  hover/details card gains a **"PR branch"** row in the mismatching case only.
  See `docs/features/sidebar.md` §§ "Side-branch PR badges", "A side-branch
  association is a badge and nothing more" and
  `docs/features/review-integration.md`.

- **Monitoring continuity across follow-up turns (current branch).** A new
  user turn now clears only turn-scoped agent/review bookkeeping and retains
  confirmed live monitor entries. The parent still shows `Working` during the
  follow-up, then returns to `Monitoring` on settle even when the provider
  emits no new progress snapshot. Both the provider-agnostic send boundary and
  `SessionStateChanged::Running` use the same `begin_turn` rule, so future
  Codex/OpenCode monitor classification gets the same behavior; their current
  manual-monitor path already sits outside the tracker and is unaffected.

- **"Monitoring" agent status for background watch loops** (PR #251). A fifth `PaneStatus` (`Monitoring`) for the state Codemux previously had no vocabulary for: an agent that finished its deliverable but is still babysitting something — a CI run, a tailed process, a PR poll. Two ways in, meeting at one choke point. **Automatic (Claude):** `task_started`'s optional `task_type` is classified against a closed watch-loop set (`monitor` / `monitor_mcp` / `local_bash` / `shell`), remembered per subagent in `SubagentDemux` and re-stamped onto every later snapshot (only `task_started` carries the field). This is the *second* classification on `SubagentSnapshot`, and it composes with the `background_task` flag from the run-settlement fix above rather than competing with it: one `stamp_task_classification` in each `translate_task_*` fn stamps both, and the tracker folds them into one `TaskClass` per id — `Monitor` when the SDK says so, `Untracked` for a background row that is not a watch loop, `Agent` otherwise. **Monitoring is orthogonal to settlement, not a replacement for it.** A watch loop never defers anything: the turn still settles on schedule (`Review` publishes, `run_finished` fires, the detached browser is released on the genuine transition), and all a live monitor changes is *which* settled status is shown. `settled_status()` is a decision table read straight off the live sets — `Permission > Working > Monitoring > Review > Idle` — so a monitor whose first snapshot arrives *after* the turn settled still lights the badge, and the moment the last one ends the pane falls to the `Review`/`Idle` it would otherwise have shown. Because a monitor-only thread owes no `Review` at all, the 600s force-settle watchdog cannot see it, and a chatty watch loop cannot re-arm the silence clock for a thread that a real subagent *is* blocking. An SDK that never sends `task_type` classifies everything as agent work and behaves exactly as before — deliberate graceful degradation. **Provider-agnostic:** runtime-only `manual_monitors` flags set by `codemux monitor start [--reason]` / cleared by `codemux monitor stop` (plus `monitor status`, and the matching `monitor_start`/`monitor_stop`/`monitor_status` socket commands), targeting the agent's own pane from injected `CODEMUX_PANE_ID`/`CODEMUX_WORKSPACE_ID` — so a terminal, Codex, or OpenCode agent gets the same badge. The two halves combine in exactly one place, `apply_manual_monitors`, on the way *out* of the state store (`Working`/`Permission` beat the flag; combining on read is what lets `monitor stop` reveal the raw status underneath). Never persisted: `retain_persistable_pane_statuses` drops `Monitoring` like `Working`/`Permission`, and flags are cleared when their pane, workspace, or archived workspace goes away. UI is deliberately calm — a new steady cyan `--status-monitoring` token with **no** pulse or spinner on the inbox card, rail dot, hover card, tab dot, palette row and overview row; monitoring cards recede like quietly-working ones, never raise the needs-you strip, and stay settleable/snoozeable. A docked `MonitoringBar` between transcript and composer carries the reason and a **Stop** button that clears the monitor set + flag and best-effort interrupts the session; Stop is durable (stopped ids are blocklisted until the next turn boundary, so a surviving detached task's later ticks cannot walk the badge back on) and works on a pane with no bound thread, with the honest limitation that once the turn has settled there is no bare-session interrupt, so a detached process can outlive the state clear. Watch loops keep their transcript card but are excluded from the `SubagentActivityBar` roster. Covered by Rust tracker/state tests, `pane-status`/`MonitoringBar`/sidebar-card vitest suites, and a headless `scripts/e2e/monitoring-status-e2e.sh` that drives `codemux serve` over an isolated control socket. See `docs/features/monitoring-status.md`.

- **Durable workspace pinning** (PR #248). Any workspace can be pinned from its
  context menu into a dedicated block above the active inbox; pinned cards keep
  static creation order, expose a direct hover/focus Unpin action, remain
  visible in the collapsed rail, and are protected from settle, snooze,
  auto-settle, and bulk parking. `WorkspaceSnapshot.pinned_at` is persisted by
  Rust and carried through archive/restore with its original timestamp intact.
  Pinning is a visibility override rather than a destructive lifecycle
  transition: an already-settled or snoozed workspace keeps that shelf entry and
  returns to it when unpinned. The override is against *parking* only — the
  settle safety net and the snooze wake sweep still clear a preserved entry
  underneath a pinned card when the agent goes live or the wake time elapses, so
  unpinning reveals the current lifecycle rather than a stale one; the cosmetic
  "Woke" pill is suppressed while pinned, since the card never left the list.
  See `docs/features/sidebar.md`.

- **AppImage bundled libraries stop leaking into child processes** (PR #250).
  Under an AppImage, `AppRun` rewrites `LD_LIBRARY_PATH`, `PYTHONHOME`, `PATH`
  and other loader/toolkit variables to point into the mounted AppDir so the
  bundled binary finds its bundled libraries — and every child inherited them,
  so host binaries linked against our bundle instead of the system. On a host
  whose libraries are newer than the bundle that fails before `main` (`cargo:
  libssl.so.3: version 'OPENSSL_3.5.0' not found`), which made `npm run
  tauri:dev`, `git`, `python3`, and workspace setup scripts unusable from a
  terminal inside Codemux. A new AppImage hygiene layer sits beside the
  existing GUI-session sanitizer and is applied on every child-spawn path
  (terminals, the pty daemon, remote pty, setup scripts, JSON-RPC children):
  path lists keep the user's entries and drop only AppDir ones (a list left
  empty is removed rather than blanked), launch markers and AppRun-overwritten
  values are removed unconditionally, and fixed AppRun literals are removed
  only when the value still matches. Everything is gated on `APPDIR`, so
  distro packages (AUR, `.deb`, `.rpm`) are unaffected — the fixup list is
  empty and every call site is a no-op. A companion fix resolves the
  `codemux.service` `ExecStart` without `$APPDIR`, since that variable is by
  design absent in any process Codemux spawned. Covered by
  `src-tauri/tests/appimage_env_hygiene.rs`. See
  `docs/features/execution.md` § "AppImage Environment".

- **The active card's PR chip matches the settled row's badge** (PR #253). The
  sidebar rendered the same pull request two ways depending on which tier its
  workspace was in — active cards drew a bordered, tinted `PR #248` box, settled
  rows a bare state-colored icon plus `#248` — so settling a card looked like the
  badge had changed when only the tier had. The active card drops the chrome and
  reuses the settled treatment, and parity is behavioral as well as visual: the
  no-URL chip uses `cursor-default` rather than `pointer-events-none` (passing the
  click through to the card root had activated the workspace from a dead chip),
  and the card root's keydown adopts the same target-scoped guard the settled and
  snoozed rows use, extracted to `sidebar-row-activation.ts`, so Enter on the PR
  chip no longer both opens the PR and activates the card. `PR_CHIP_TONE` stays
  put — the Agent Chat Context Row still wants a labeled, bordered action. See
  `docs/features/sidebar.md`.

- **The workspace hover card keeps up with the pointer** (PR #255). The card
  waited 350ms on every row and then spent 150ms fading, zooming and sliding 8px
  into place, so a deliberate hover took half a second to become readable and a
  sweep down the sidebar re-paid the full cost on every row — with the row just
  left still holding its card up through a 120ms close delay. The first-hover
  delay is halved to 150ms and a **shared group phase** (a module-level store,
  since Radix's HoverCard has no provider to hang it on) opens every subsequent
  card with no delay and no entrance animation for 400ms past the last close,
  superseding the previous card the moment the next one opens. The close delay
  stays (at 100ms) so the pointer can cross the offset gap into the card to copy
  a path or branch, but a row-to-row sweep no longer pays it; the 8px slide is
  gone and the zoom softened to 98%, since a card still visibly travelling after
  it appeared is what read as lag. See `docs/features/sidebar.md`.

- **Send-in-thread scroll feel** (PR #249, follow-up to #247). Two behavior changes to the new-turn scroll contract, both aimed at how a follow-up send in a long thread *feels*. First, **the anchor now outlives the turn**: it persists until the next send replaces it, a rollback clears it, or the thread changes. #247 expired it on the falling edge of `streaming || isSending`, which unmounted the reserved end space in one frame and visibly yanked the parked prompt from the top to mid-viewport at the exact moment the reply finished; now nothing on screen moves at settle, at the deliberate price of blank space persisting below a completed reply. The two hazards that motivated expiry got targeted fixes instead: "which send nonce is already positioned" moved into a pane-owned ref (`sendAnchorPositionedNonceRef` → `positionedNonceRef`) so a `MessageList` remount under a live anchor re-reserves space without re-parking, and `anchoredEndSpace.onSizeChanged` re-runs the advance decision so no-data-change layout growth (a late-loading image) still reveals the tail while the built-in pin is off — plus a ≤2px offset restore makes spacer resizes imperceptible to a free-scrolling reader. Second, **the send positioning is an animated glide** (`scrollToIndex({ animated: true })`) with a settle handshake — `scrollend`, or a 750ms fallback timer where the event is unsupported — that re-pins the landed offset instantly and gates the stream-advance effect so a fast first token cannot cut the glide short; `prefers-reduced-motion` keeps the instant placement. Smaller feel parity: the edge signal for the pill and follow re-claim is now `isNearEnd` (half a viewport) instead of hairline `isAtEnd`, and the pill's return glides too. Frontend-only; the pure geometry in `send-scroll-state.ts` is untouched. See `docs/features/agent-chat.md` § "Transcript scroller" → "The new-turn scroll contract".

Shipped in `v0.17.0` (2026-08-05) — eight merged PRs (#239–#246) plus the
directly-committed new-turn scroll contract (PR #247) and the floating-chrome
refinement commits that landed alongside PR #245, grouped by subsystem below:

- **New-turn scroll contract for the chat transcript** (PR #247). A composer submission is now an explicit navigation intent rather than a data update. `AgentChatPane` issues a `sendAnchor` (`{ clientNonce, nonce }`) in the same batch as the optimistic `appendUserMessage`, reusing that bubble's existing correlation token; `MessageList` resolves the matching `user_message` slot **by nonce** (last match wins) — not by last index, which queued follow-ups and control rows break — feeds it to LegendList's `anchoredEndSpace` (`anchorOffset: 16`), and positions the row from the `onReady` measurement callback with a `scrollToIndex` (instant in #247; an animated glide since PR #249), retrying per frame rather than assuming layout finishes in N ms. Three states replace the old one-shot scalar signal: `following-end`, `anchoring-turn`, `free-scrolling`. While an anchor is mounted the built-in `maintainScrollAtEnd` is disabled and an effect advances by exactly `scrollDeltaToRevealEnd`, so the prompt stays parked near the top while the turn fits and moves only enough to reveal the growing tail. Follow is released **only** by `wheel`, `touchmove`, a `pointerdown` that targets the scroll container itself (a scrollbar drag — presses on rows, so plan-accept/approval/expand clicks and text selection, deliberately do not count), or subagent-jump navigation; a generation counter invalidates every in-flight continuation at once, and scrolling back to the edge re-claims follow. The anchor clears on failed-send rollback and on thread switch; since PR #249 it **survives the turn settling** (see that bullet — #247's falling-edge expiry was the source of a visible settle-time viewport yank), and clearing never re-claims the viewport from a reader already browsing history. "Jump to latest" is shown on a 150ms trailing debounce and hidden immediately, so it no longer flashes during mount/layout settling or sticks on during programmatic anchoring. Geometry lives in the pure, unit-tested `send-scroll-state.ts`. Frontend-only; the previous `scrollToBottomSignal` scalar and its single `scrollToEnd` call are gone. See `docs/features/agent-chat.md` § "Transcript scroller" → "The new-turn scroll contract".

- **Provider-correct cross-provider skills** (PR #243). The cwd-scoped inventory merges readable ancestor filesystem definitions with Codex `skills/list` and OpenCode `/skill` catalogs, computes one projection per target provider, and isolates adapter errors. Slash selections carry exact ids through IPC and are backend-revalidated; unique names keep `/name`, collisions use qualified tokens, Codex gets structured skill items, and portable fallbacks preserve body provenance + base directory. Settings switches govern Codemux availability only and state that provider-native discovery remains independent. Claude's provenance-insufficient SDK list stays in the provider command group. The prior first-wins name/body injection behavior is gone. See `docs/features/agent-chat.md` and `docs/archive/cross-provider-skill-support.md`.

- **Legacy standalone orchestration runtime retired** (PR #246). The old workspace type was unreachable from a cold start and had no durable user entry point. Its frontend (`src/components/openflow/`, the store, hooks and graph helpers), Tauri commands/events/types, Rust runtime (`src-tauri/src/openflow/`) and agent-spawn path, virtual-display/sandbox prototype (`execution/virtual_display.rs`), history table, reserved app-port range, tests, and active docs are removed — 106 files, ~13.5k lines deleted. Layout loading maps unknown retired workspace kinds to a compatibility sink (`WorkspaceType::Removed`) and strips their sessions before state restoration; schema **v10** drops the stale `openflow_history` table. The current Agent Chat workflow orchestration surface (`docs/features/workflow-orchestration.md`) is a separate, still-live feature and is unchanged.

- **Chrome refinement batch** (PR #245 plus the direct commits that landed with it). The full-width **workspace context bar is deleted** — component and tests gone, no mount in `app-shell.tsx`; the background-browser affordance it carried moved into the terminal pane header (`background-browser-indicator.tsx`, mounted by `PaneNode.tsx`), so browser activity stays discoverable without reserving 42px under every terminal. The GUI title bar became **frameless floating islands**: no full-width surface or divider, sidebar/workspace/right-panel all reach the physical top edge, and the two 32px islands gain an opaque raised surface only when a transcript has scrolled under them *and* an island intersects the centered reading column (new `src/lib/titlebar-content-under.ts` registry). Terminal pane chrome went transparent — a sole-root terminal drops the redundant `Terminal` label and shows only a compact cwd chip when the live cwd differs from the workspace root; this treatment is **flag-independent**, so a terminal looks identical in legacy and GUI mode. A new exported `useTitlebarOverlay()` predicate (`src/hooks/use-gui-chrome.ts`) is now the single gate every top-edge collision clearance must use, fixing dead bands with the flag off; both drag layers became desktop-only so the web client's right-panel tab row is no longer covered by a pointer sink. See `docs/features/workspace-context-bar.md`, `docs/features/gui-chrome.md`, `docs/features/terminal.md`.

- **Local screenshot references in Agent Chat** (PR #239). Absolute PNG/JPEG/GIF/WebP paths returned in assistant Markdown — normal links or image syntax — now render as labelled preview cards instead of dead underlined filesystem anchors, and click through to a near-fullscreen lightbox. Desktop uses the Tauri asset protocol; web-remote and the browser dev mock fall back to a dedicated, absolute-path-only image reader (`agent_chat_read_local_image`) capped at the same 25 MB ceiling as chat attachments. Missing temp files degrade to a stable unavailable card. See `docs/features/agent-chat.md` § "Local screenshot links in chat".

- **Codex resume context preserved across restart** (PR #241). `thread/resume { threadId }` is now resolved from the persisted cursor (`codex_thread_id_from_resume_cursor` accepts `threadId` then the generic `resume` wrapper), so the prior Codex thread's complete model-visible history — **including image inputs** — survives an app restart and is available when the user clicks **Continue run**. The Continue turn itself remains a plain follow-up and does not duplicate the prior attachments. The previous "falls back to a fresh session because Codex's start-time cursor carries no extractable id" claim was wrong and is gone from the docs.

- **Text-selection colors standardized** (PR #240, plus the editor follow-up). Two new literal design tokens — `--selection-background` (`var(--accent-ember)`) and `--selection-foreground` — drive a single root-scoped `::selection` rule, so every surface highlights identically instead of inheriting per-component browser defaults. Renderers that own their own coloring opt out explicitly: xterm.js needs nothing because `.xterm` is `user-select: none`, and CodeMirror document lines restore `color: currentColor` so **selected code keeps its syntax colors**. See `docs/reference/DESIGN-SYSTEM.md` § "Text Selection".

- **Composer focus treatment** (PR #242). The Agent Chat composer no longer signals focus with a brightened border around its large rounded rectangle; the border is deliberately identical at rest and on focus, and focus is carried by surface tint plus elevation (`focus-within:bg-muted/60` + a soft drop shadow). See `docs/reference/DESIGN-SYSTEM.md` § "Focus Treatment".

- **Right-panel toggle icon mirrored** (PR #244). The right-panel toggle uses the `PanelRight` glyph in both chrome modes, mirroring the sidebar's `PanelLeft`, instead of the unrelated `FileDiff` icon. This is the one deliberate exception to the "legacy chrome is byte-identical with the flag off" rule — the legacy `TabBar` toggle changed too.

The browser pane is functional but still being hardened for native-feel interaction and lifecycle reliability.

The repo structure is clean and domain-split:

- `src/` is the React + Tailwind + shadcn UI and Tauri IPC layer
- `src-tauri/` is the Rust app/runtime layer
- `sidecar/claude-agent/` is the Bun-compiled TypeScript subprocess that hosts the Claude Agent SDK

## Solid — Daily-Drivable Features

### Workspace & terminals
- Workspace shell, sidebar, workspace sections with color coding and drag-drop
- Multi-session terminals with xterm.js, WebGL renderer **gated by a hardware-GL probe** (`webgl-renderer-probe.ts`: DOM renderer on software-rendered WebGL and on Linux WebKitGTK; WebGL on real-GPU WebView2 / macOS; automatic DOM fallback on GPU context loss / missing WebGL2) + Unicode 11 widths, kitty protocol, low-latency pane input
- Hidden-pane terminal pause to eliminate cross-workspace typing lag
- Tab bar with terminal, browser, editor, and diff tab types
- Pane splits, resize, drag-swap, close — split panes inherit the workspace cwd (`v0.7.5`)
- **PTY producer back-pressure — engaged on the live path** (issue #73; daemon-side plumbing since `v0.7.5`): the live `TerminalPane`'s write pump tracks queued bytes and calls `pause_pty_output` above the HIGH watermark (16 MiB) / `resume_pty_output` below LOW (4 MiB, hysteresis), so a fast producer (`yes`, a verbose build, a runaway agent) blocks on the kernel PTY buffer instead of ballooning the renderer queue or overflowing the `pending_output` ring. Both spawn paths honor the pause: daemon-backed sessions via the `SetFlowPaused` wire request, in-process sessions via a per-session `flow_paused: Arc<AtomicBool>` polled by `batched_reader_loop` (previously a deliberate no-op). Self-heals everywhere — the flag clears on attach/detach, a 10 s `FLOW_MAX_PARK` backstop force-resumes a wedged pause, and the renderer resumes on unmount. The throttled write pump remains the consumer-side complement. See `docs/features/terminal.md`.
- **Terminal workspace-switch performance**: the live terminal path is the per-mount lifecycle in `src/components/terminal/TerminalPane.tsx` (xterm constructed on mount, disposed on unmount — only the active workspace/surface is ever rendered). Switching no longer freezes because all xterm writes (disk-scrollback restore + the `attach_pty_output` reattach replay + live output) drain through a throttled, byte-budgeted write pump (`src/components/terminal/terminal-write-pump.ts`) that yields between batches, and the wasted serialize of alt-screen TUI buffers (Claude Code, lazygit, vim, btop) is skipped on unmount. The earlier persistent-xterm cache (`terminal-cache.ts`, shipped `14735bf`) was rolled back in `2baa42f`; it is retained but **disabled / not wired** (see its banner) pending a possible future flag-gated revival, so `useTerminalCacheGc` / `useTerminalThemeSync` are no-ops today.
- **Session persistence**: terminal scrollback save/restore across restarts (Windows-only backend backstop in `scrollback::flush_cache_to_disk`), adapter-based resume for CLI tools (Claude Code `--resume`/`--continue` via hook-captured session IDs)
- **Persistent PTY daemon** (`pty_daemon::server` + `client` + `supervisor` + `manifest`): every shell spawn now routes through a detached `codemux pty-daemon` subprocess so agents survive app close. On relaunch the supervisor adopts the running daemon and reattaches live sessions. **Default-on**, no setting; `CODEMUX_DISABLE_PTY_DAEMON=1` is the only escape hatch. Graceful fallback to the in-process portable-pty path on every error site, plus a 3-failures-in-60s crash circuit breaker that disables the daemon path for the rest of the process lifetime. Unix only; Windows still uses the in-process path until the named-pipe IPC is wired. **Since `v0.7.9`** the daemon idle-reaps after 1h with zero sessions, with a lock-held live-session recheck before exit.

### Git & GitHub
- Git worktree-based workspaces (create from new/existing branch, import orphans, derivative-branch picker with recency)
- New-branch creation fetches `origin/<base>` first (scoped best-effort fetch, 10 s cap, offline/local fallback, runs on the blocking pool) so branches start at the remote tip, not a stale snapshot — desktop and headless daemon share the policy
- Changes panel in right sidebar (stage/unstage/commit/push, inline per-file diffs, AI commit messages, Open-PR button on toolbar, AI merge resolver entry)
- Full-pane diff viewer tab (unified/split layouts, section filters incl. `against_base`, hunk/file navigation, focus mode)
- Review tab (renamed from PR tab, React Query refactor): PR creation, header, reviews, checks, deployments, merge controls
- Sidebar PR status icon per workspace with stale-clearing on branch switch and DRAFT collapse
- **Default-branch detection** drives the sidebar branch pill: seed from `origin/HEAD`, follow live remote-branch changes, and the derivative-branch picker drops the phantom `origin/<name>` rows so users never pick a remote-only ref by accident
- "Checkout default branch" workspace action
- Git sidebar enrichment (branch, ahead/behind, diff stats, PR badge) with non-blocking activate + visibility-based gate
- Sidebar ahead/behind arrows refresh against fresh remote refs
- Auto-transition to main workspace after merge+delete
- Merge-into-base runs on the blocking pool so it cannot freeze the GUI; uses `update-ref` for worktree compatibility
- Hide stale merged-PR pill on long-lived branches
- Review tab unfreezes on repos with thousands of PRs (paginated fetch)
- All git/gh shell-outs moved off the GTK main thread to keep IPC responsive

### GitHub issues
- Link issues to workspaces, issue picker in creation dialog, sidebar display with detail popover, auto-branch naming from issue, prompt auto-injection of issue context
- CLI: `codemux issue list/view/link`, control socket commands

### Browser
- Screenshot-driven Chromium session backed by `agent-browser` v0.24.0 (pure Rust, direct CDP)
- Browser pane in pane layouts; address bar, refresh, home, external-link
- Per-workspace stream sessions keyed by `workspace_id` (PID-tracked daemons, single canonical key, atomic teardown, symmetric `TcpListener` bind probe, reactive `stream_url` reconnect on the frontend)
- Dynamic stream ports (9223–9299) for concurrent worktrees
- Stealth Chromium flags + realistic user-agent string
- Browser data management in Settings (clear cookies, clear all data, view data size)
- Inspector panel for debugging web content
- **Viewport presets**: `codemux browser viewport <mobile|tablet|desktop|WxH|reset>` resizes the actual viewport via CDP so CSS media queries fire and screenshots capture at the simulated dimensions. MCP exposes `browser_viewport` + `browser_viewport_presets`.
- Browser stream stability fix shipped (commit `7e36420`): unified port keying, hardened daemon lifecycle, dropped dead workspace_id alias lookups. Eliminates the silent stream failure that appeared after multiple concurrent worktrees used the browser.
- **Native-feel interaction pass** (`v0.9.2`, issue #99): drag-to-select text (mousemoves now carry the held CDP button — the daemon derives Chromium's "button held" modifier from it — plus pointer capture so out-of-pane releases land), hover effects (coalesced mousemove forwarding), double/triple-click selection (chained `clickCount`), right/middle-click, a live cursor mirroring the remote page via throttled `elementFromPoint` probes, and a host-clipboard bridge (Ctrl+C/X mirror the page selection to the host clipboard; Ctrl+V inserts host text via chunked `execCommand` evals). The false "reconnecting" cycle is gone: quiet screencasts (static pages emit no frames) are verified against the daemon's HTTP `/api/status` before reconnecting, reconnects keep the last frame visible behind a corner pill, and exhausted fast retries fall back to a 10s self-heal loop. New `src/components/browser/stream-protocol.ts` (+33 unit tests); page-script evals are minified and paste is chunked at 120 chars because the daemon's HTTP reader only sees single-segment requests (~1.4KB ceiling, measured). Dev mock now seeds a `browser-demo` workspace wired to `ws://127.0.0.1:9777` for end-to-end pane work in plain-browser dev.
- **Background browser in Agent Chat GUI mode** (`v0.12.0`, PR #138): in GUI mode (Agent Chat on), an agent-opened browser stays detached instead of splitting the chat into a pane — an inline conversation chip plus the active terminal's compact header indicator surface it, and a floating peek overlay (`BrowserPeekOverlay`) previews the live stream without resizing the work surface, with a one-click promote to today's split-pane view. The former full-width workspace context bar is removed; workspace detail remains in the sidebar/hover card and Agent Chat Context Row. The flag-off path keeps the split-pane behavior. See below.

### Workspace creation
- Multi-step creation dialog with task description, branch selection, agent preset
- **Model + reasoning selection before launch**: a model pill next to the agent picker. It appears for any preset whose command launches an already-modeled CLI (`claude` / `codex` / `opencode` / `gemini`) — detected from the command's binary token, so new presets light it up automatically. Models come from the same capability harvest as the Beta chat picker (OpenCode live-harvested; Claude/Codex maintained; Gemini a small static list); reasoning is `--effort` (Claude) / `-c model_reasoning_effort` (Codex); Claude also gets a context-window row (1M via the `[1m]` model-id suffix). The picker adapts: a flat list for short rosters, search + favorites + sub-provider grouping once the list is long. "Default" emits no flag, so untouched workspaces behave exactly as before. Backend injection lives in `agent_capability::apply_model_selection` (strips a baked-in flag so an explicit pick always wins) and is threaded through `apply_preset` / `create_worktree_workspace`.
- AI-generated branch names from task description
- GitHub issue / PR linking with branch auto-fill
- **Paste clipboard images directly into the prompt input** (in addition to the existing file picker)
- File attachments appended to agent prompt
- Project onboarding flow with package manager detection and setup script configuration
- Orphan worktree detection and import
- Derivative-branch picker (icons, recency, worktree tab)
- Lazy workspace creation (paired with the Agent Chat GUI flag, default on): sidebar-plus and boot-into-Home open a client-side chat draft instead of eagerly materialising a workspace; the draft is promoted on first message send

### Workspaces overview & cross-device sync
- **Account-wide Workspaces overview** (`v0.6.1`): full-screen overlay opened from the sidebar (`Workspaces` button under `Automations`) listing every workspace this account tracks — local + every host pushed-to + every sibling device on the same account. Filters (search, project, device, status, sort), per-row actions (open, copy branch, rename, push to any host, pull back, delete), agent-state status dots shared with the sidebar, hover-reveal action menu. Configured-host buckets stay visible even when empty so a device the user has just added shows up immediately, not only after the first workspace lands on it (`v0.7.x`).
- **Cross-device workspaces sync**: `workspaces_sync.rs` mirror of `hosts_sync` / `automations_sync` — every create / rename / push / pull / delete propagates through `/api/workspaces` on the shared API server (Postgres `codemux_workspaces`) on a 30s loop, scoped per-user via Better Auth bearer tokens. Sibling-device rows render as dashed cards in the right bucket. Close-workspace paths reconcile + push the sync row immediately so the overview never briefly mis-tags a just-closed workspace as "lives on another device" (`v0.7.x`).
- **Asymmetric auto-publish from `codemux-remote` hosts** (`v0.7.x`): a new `hosts_inventory.rs` poller SSHes each configured host every ~60 s, runs `codemux-remote workspace list` (a new CLI subcommand that reads the daemon's SQLite directly — no running daemon required), and reconciles the result into `workspaces_sync` as sibling-only rows keyed by `(host_server_id, origin_uid)`. The existing 30 s push tick then uploads them to the cloud, so a workspace an agent creates on a host (via the MCP `workspace_create` tool or a manual `codemux-remote workspace register`) surfaces in every dev device's overview within ~90 s — no explicit push from the laptop required. Schema is additive (`ALTER TABLE workspaces_sync ADD COLUMN origin_uid TEXT`). The asymmetry is deliberate: dev devices keep manual push/pull (user agency), hosts auto-publish (always-on, exist to be used).
- **First-class project identity + `main`/`worktree` kind** (shipped in `v0.7.4`): a deterministic `project_uid = UUIDv5(canonical git remote ?? project_root)` (in `src-tauri/src/project_identity.rs`) gives every checkout of the same repo a stable identity across devices/hosts with zero coordination. Stamped at the source (daemon `WorkspaceStore::create` + boot sweep; desktop `set_workspace_project_root`), threaded through the host-inventory poller and the cloud `codemux_workspaces` round-trip (server-authoritative on pull), and consumed by the overview (groups by `project_uid`, renders a `main`/`worktree` badge) and the pull-conflict guard (exact `project_uid` match). Completed implementation record: `docs/archive/project-identity.md`.
- **Sibling-device adoption** (Phase 3): "Pull to this device" on any sibling row — host-backed via the existing `workspace_pull_back_impl` rsync flow when both devices share a configured host, clone-from-git fallback for rows that have a `project_remote` but no shared host (creates a fresh server_id; both devices end up with independent copies sharing the git remote).
- **Pull-conflict guard** (`v0.7.x`): `AdoptionPreview.same_branch_project_exists_at` populates when **another local workspace** on this device matches `(basename(project_path), git_branch)` of the previewed remote row; the dialog renders `SameBranchProjectBlock` with an "Open the existing workspace" CTA and hides Pull, so a pull can't silently clobber work the user is already doing locally.
- **Safety guardrails** (Phase 4): every push gates through `ConfirmPushDialog` with a per-host "don't ask again" affordance; every successful push, pull, and adoption surfaces a 10-second `Undo` toast that fires the reverse action.
- **Cross-device divergence detection** (Phase 4c): `git_head_sha` syncs through the API; when the same workspace exists on multiple devices via clone-adoption and their HEADs diverge, both rows show an amber `diverged` chip with a tooltip suggesting push or pull to share.
- **Repo-unit sync — repo-root protection + labels** (`v0.7.8`): cross-device sync treats a git repo (root + worktrees) as one shared-history unit instead of cloning a default-branch checkout into a divergent full copy. A protected default-branch root (`WorkspaceSnapshot.protected`, stamped divergence-safely by `crate::git::is_protected_repo_root`) shows a `repo root` badge and can only be closed/detached, never deleted-as-worktree; sibling `RemoteRow` `main`-kind rows render `repo root` too. A legacy divergent full copy (`divergent_copy` — a `.git` directory in the worktrees tree) shows an amber `standalone copy` warning chip. Adoption lands a `main` row at `~/.codemux/projects/<repo>` via `create_synced_root_shell`; push/pull put a repo root under `~/.codemux/projects/<repo>` on the host (`conventional_remote_root_path`) with a legacy `worktrees/` fallback. Snapshot-local only (not synced). See `docs/plans/repo-unit-sync.md`.
- **Multi-device robustness pass** (`v0.7.9`): **project-first pull** with a real protected root (local-only `default_branch` column threaded daemon→poller→sync→TS; `resolve_default_branch`/`ensure_origin_head`; `workspaces_adopt_project` materialises the root then recreates each worktree under it — the overview's **"Pull project"** button); **serialized adopts** via a per-`server_id` async creation lock (`acquire_adopt_lock`) so a double-clicked Pull or a poller race can't make duplicate shells; **client-side `dedupe_sibling_rows`** collapse of cross-device duplicate sibling cards (keeps the canonical `server_id`'d row, tombstones the rest); **daemon-side one-repo-root-per-project** (`collapse_main_for_uid` + boot `normalize_main_workspaces` sweep; `WorkspaceStore::create` collapses after inserting a `main` row); **uid-keyed collision-safe host paths** (`<basename>-<short-uid>` so `acme/api` vs `widgets/api` stop colliding, exact legacy basename layout when no uid is known — **issue #65** finished the re-key on the local side: adoption landing paths are now uid-keyed too (`adopt_root_landing`/`adopt_worktree_landing` + a `choose_landing` basename read-fallback, shared by the preview and all three adopt paths so re-pull stays idempotent), and the Claude `.jsonl` session-continuity sync derives the remote dir from the workspace's actual on-host path (`resolve_remote_cwd`) instead of recomputing the basename, so conversation history syncs for uid-keyed workspaces); and a **non-destructive `workspaces_reconcile_copy`** action (detach-card-only, files left on disk, refuses on uncommitted/unpushed work) plus an overview **"Reconcile copy…"** menu item. See `docs/features/workspaces-sync.md` § "Robustness hardening", `docs/plans/repo-unit-sync.md`.
- **SSH tunnel health in the UI** (`v0.7.9`): a dropped tunnel (sleep/wake, WiFi flap) no longer looks like a frozen workspace — `spawn_tunnel_status_forwarder` bridges the supervisor's `TunnelStatus` watch channel to a `tunnel-status-changed` event, a zustand `tunnel-status-store` (app-root `useTunnelStatusEvents` hook) feeds a sidebar pill: amber "Reconnecting…" while retrying, red "Connection lost — re-push" once the circuit breaker trips. See `docs/features/remote-hosts.md` § "Tunnel health in the UI".
- **Elapsed-time indicator** (Phase 4d): when a push or pull takes longer than 2s, a compact `12s` pill appears next to the spinner in both the sidebar row and the overview row so the user knows the operation hasn't stalled. Powered by `workspacePushPullStartedAt` in `app-store`.
- **First-run welcome banner** (Phase 5): three state-aware variants (brand-new, device-configured, has-siblings) — dismissable, persisted in localStorage, never re-shows. Bundled with the "how it works" popover off the overview header.
- **PR-state sidebar icon swap**: when a worktree workspace has a PR, the sidebar row's leading icon becomes the PR-state-colored icon (open=green, merged=purple, closed=red, draft=gray) and the trailing duplicate PR pill is gone. Click opens the PR on GitHub.

### Search & navigation
- Keyword search (Ctrl+Shift+F via rg) and file name search (Ctrl+Shift+P via fd)
- Command palette (Ctrl+K, fuzzy search across all actions)
- Local lexical indexing (`codemux index build/search`)

### Notifications
- D-Bus desktop notifications via `notify_rust::Notification` (Normal urgency so daemons auto-dismiss)
- Sidebar notification section with unread badge counts
- Workspace alerts with severity levels
- Desktop notification toast + chime when an off-screen agent finishes
- Notification sound playback wired on all three platforms (Linux: `paplay` + freedesktop `complete.oga`; macOS: `afplay` + Glass.aiff; Windows: PowerShell SystemSounds)
- **Per-worktree mute** (`set_workspace_muted` + sidebar context-menu toggle): silences agent-completion notifications for a specific workspace without touching global sound state; muted state surfaces as a sidebar row icon

### Auth & sync
- GitHub OAuth, email/password with email verification, encrypted token storage (AES-256-GCM, machine-bound key)
- **Auth module split**: `auth/{mod,api,derivation}.rs` with the `AuthSecret` typed boundary on the API helpers (compile-time guard against raw-password leaks)
- **Auth derivation** (Step 10): `derive_auth_secret(password, email)` produces the server-visible `AuthSecret` (sent to Better Auth in place of the raw password); the sibling `derive_login_credentials` additionally derives a client-only `EncryptionKey` (32 raw bytes, never leaves the device). Argon2id (m=64MiB, t=3, p=4) with email-bound salt, fanned out via HKDF-SHA256 to two domain-separated secrets. Cross-product byte-identical with Vexis via the shared `codemux-api-*` HKDF labels — pinned in CI. **Since PR #112 the `EncryptionKey` half is no longer used by Codemux** (skills moved to server-side storage): `derive_login_credentials` + its Vexis hex pin are retained in `auth/derivation.rs` only as a protocol canary (Vexis still uses the key half for `voice_*`), and drift still fails CI.
- **Server-side skills sync** (Step 10; server-side since PR #112): cross-device sync of user-authored skills under `~/.codemux/skills/`, `~/.claude/skills/`, `~/.codex/skills/`, `~/.agents/skills/`, `~/.opencode/skills/`, `~/.config/opencode/skills/` (the scan roots and the sync path table are kept in lockstep by build-failing guard tests). Skills are stored **server-side** — the skill `name` + `content` travel as plaintext and the server persists them in plaintext columns (protected by encryption-at-rest, the same model `user_settings` sync uses) — so there is **no client-held encryption key and GitHub OAuth (SSO) users sync with no password prompt**. Push triggered by file watcher (1.5s frontend debounce on top of the watcher's 300ms), 5-min periodic when the window is visible, on any signin, or via manual "Sync now". Last-write-wins by `updated_at`. A one-time in-place migration rewrites pre-server-side ciphertext rows to plaintext via PUT to their existing `remote_id` (no duplicate rows, no data loss; unreadable legacy rows return an empty name and are skipped). Settings → Account → Sync is now a two-state dashboard (ready status + Export/Import, or a sign-in hint) — the password forms + reset-sync-password dialog are gone. The server change (additive `name`/`content` columns + plaintext `/api/skills` routes) is staged + test-DB-verified but not yet deployed. See `docs/features/skills-sync.md`.
- Per-user synced settings with server sync, offline cache, and dirty flag

### Presets & launchers
- Terminal presets with quick-launch bar (Claude Code, Codex, OpenCode, Gemini, Antigravity, Copilot, Cursor Agent, Amp, Grok, Droid, Mastracode, Pi, Shell, Chat Agent) — each agent preset launches in its CLI's skip-permissions/YOLO mode
- Pin/unpin to control bar visibility
- Auto-run on workspace creation or new tab
- Partial-materialise recovery for preset DnD; new-tab preset launch
- Preset failures surface as 8-second sonner toasts (was silently `.catch(console.error)`)
- Agent context injection for Claude / Codex / Pi / Gemini presets (uses `$VAR` on POSIX, `$env:VAR` on Windows)

### File tooling
- File tree panel (right sidebar, lazy-loaded, `.gitignore`-aware, opens in built-in editor or external editor)
- Built-in file editor with CodeMirror 6, syntax highlighting for 20+ languages, markdown preview (image loading in markdown and as standalone files)
- IDE integration: detect up to 19 editors and open the workspace from the title-bar launcher or workspace context menu. Launcher and submenu partition entries into labelled sections (VS Code family / Modern editors / JetBrains / Other) when more than one family is detected. On Windows, `find_editors()` uses the `which::which()` Rust crate plus `%LOCALAPPDATA%\Programs` / `%ProgramFiles%` fallbacks for VS Code / Cursor / VSCodium / Zed / Windsurf / Trae / Lapce; JetBrains stays PATH-only via Toolbox shims.
- Port detection (auto-scan, sidebar display, open in browser). Windows path filters system processes (`svchost.exe`, `System`, `lsass.exe`, etc.) so the sidebar doesn't surface 16+ kernel-owned ports. **Docker-published container ports** for open codemux worktrees surface under a dedicated **Docker** group (via `docker ps`, scoped by the compose `working_dir` label) — recovering ports the Linux `/proc` scan can't see because `docker-proxy` runs as root; kill is hidden for these rows. See `docs/features/ports.md`.

### Theming
- Neutral dark shell theming with Omarchy accent sync
- Sans-serif chrome (DM Sans), monospace terminals
- Terminal colors fully theme-reactive via CSS variables + MutationObserver
- Fallback Tokyonight-inspired theme when Omarchy unavailable
- **Refined-minimal sidebar redesign**: slim Changes panel + ADE-native right sidebar, consolidated title-bar menu into the sidebar footer, aligned preset-bar icons + tab-bar drop indicator, sidebar workspace rows redesigned with new hover/active states
- **Refined-minimal Settings panel**: every section now uses shared primitives, back-button hit area widened, panel layout matches the sidebar aesthetic

### Agent Chat (default interface — opt-out back to CLI via Settings → Interface)
- Full multi-provider chat pane with streaming, approvals, mode pills (Ask / Allow always / Plan / Debug), and permission-mode restart
- **Three providers** behind one unified picker: **Claude** (Claude Agent SDK via Bun-compiled `claude-agent` sidecar), **Codex** (`codex app-server` JSON-RPC), **OpenCode** (federated — Rust-direct HTTP against a managed `opencode serve` child, 100+ upstream providers funneled through one rail entry). See `docs/features/multi-provider-chat.md`.
- Unified provider+model picker (2-column popover: provider rail + searchable model list); favorites with `localStorage` persistence
- Codex finally GUI-selectable (was hidden behind a stale `ENABLE_PROVIDER_PICKER` flag pre-Step 12)
- Session history selector + draft surface chrome polish + plan proposals + AskUserQuestion panel + thinking indicator
- **Attachments via `+` and `@`**: files, folders, GitHub issues + PRs, images via paste / drop / + picker. Inline chips, send-time injection, expand, caps, gif guard, chip tooltips
- Slash command popup + Shift+Tab mode cycling
- Cross-provider **skill system** (provider catalogs, ancestor discovery, exact collision-safe invocation, watcher, Codemux availability, per-target compatibility)
- Per-tool body rendering in approval blocks (read/write/edit/grep/etc.)
- **MCP host runtime**: Codemux discovers user-installed MCP servers across Codemux / Claude / Cursor paths, spawns each child once, dedupes identical configs, exposes tools to the Claude SDK via an in-process facade with dynamic `setMcpServers` refresh. Settings panel and `+` popup surface enable/disable + status badges + tool list modal + 50-tool cap warning. Codex MCP support planned for Step 11 via HTTP gateway.
- Permissions settings page with per-tool body rendering
- Wired Debug mode pill with marker cleanup flow
- Plain-quit on interface toggle flip (no auto-restart)
- **Run checkpoints (issue #80, opt-in)**: background working-tree snapshot at session start (shadow ref via temp index — zero first-token latency, user's index/worktree/stash untouched), restore button in the pane header (confirm dialog, disabled mid-turn), Settings → Agent toggle, per-thread bookkeeping in `agent_chat_checkpoints`, ref pruning. See `docs/features/agent-chat.md` § Run checkpoints + `docs/archive/agent-run-checkpoint.md`
- **GUI redesign on shadcn chat primitives** (`v0.10.0`, PR #119): Streamdown assistant markdown, a collapsible `reasoning` "Thought for Ns" block, tool-group/diff/task cards, a 760px anchored `+`/`@`/slash command-menu composer, provider-marked assistant avatars; plus a **transcript navigation trail** (PR #120 — left-gutter turn tick rail, hover preview + click-to-jump). The transcript later moved from the original shadcn MessageScroller to LegendList windowing. See `docs/features/agent-chat.md`.
- **Activity Stream** (`v0.11.0`, PR #124): contiguous reasoning + tool-call runs fold into one collapsed `activity` slot (amber "Working" spinner + `N done · M running` while streaming; green-check summary + `N steps · duration` + Details toggle when settled); approval-gated tools / TodoWrite / plan / prose still break the run. `ToolGroupCard` retired. See `docs/features/agent-chat.md` § "Activity block".
- **Subagent view (cross-provider)** (`v0.11.0`, PR #125): a Subagents orchestration card (one row per subagent — status, name + model, live activity line, elapsed + tool count, inline peek, Enter) + an in-pane read-only drill-in into a subagent's own sub-transcript, driven by one canonical `SubagentUpdated`/`SubagentSnapshot` backend model emitted by all three adapters; subagent events ride the parent thread so cards survive restart with zero migration. See `docs/features/agent-chat.md` § "Subagent view (cross-provider)", `docs/archive/subagent-view.md`.
- **Restart auto-resume + persisted picker config** (`v0.11.0`, PR #130): `ensure_live_session` rebuilds a dead provider session on the next send/respond after an app restart (same `thread_id`, `resume_cursor`); per-thread `model`/`effort`/`context_window`/`permission_mode` persist across restart (schema v8). See `docs/features/agent-chat.md` § "Session lifecycle".
- **Syntax-highlighted code blocks** (shipped in `v0.15.6`, PR #216): Shiki highlighting for fenced blocks, themed from the live terminal ANSI palette so chat, editor, and terminal agree on colors and a theme switch recolors chat live; line numbers off, Settings → Appearance → "Wrap code in chat" (`chat.code_wrap`, default off). See `docs/features/agent-chat.md` § "Code blocks in chat".
- **Stale-request expiry** (shipped in `v0.15.6`, PR #220): an approval or `AskUserQuestion` answered after its provider session restarted terminalizes with an explanation instead of erroring or spawning a replacement session; OpenCode opts out because its permission state lives in the external HTTP server, and an already-`resolved` request can never be expired by a duplicate respond. See `docs/features/agent-chat.md` § "Pending requests are not conversation history".
- See `docs/features/agent-chat.md` for the canonical feature breakdown

### Infrastructure
- Global overlay manager (single overlay at a time)
- MCP server exposing **55 tools** via JSON-RPC 2.0 (the prior 52-tool inventory plus `workspace_archive`, `workspace_unarchive`, and `workspace_archive_list`)
- CLI and socket control (Unix socket on Linux/macOS, named pipe on Windows). Control-endpoint errors now surface instead of being swallowed.
- Local project memory (`codemux memory show/set/add`, `codemux handoff`)
- Auto-update via Tauri updater (Linux AppImage + Windows NSIS, signed with the same Ed25519 key, shared `latest.json`)
- Onboarding skip affordance + re-trap fix
- Dev builds isolated from installed release (separate data dirs)
- **Remote hosts + workspace push**: Settings → Hosts with real `hosts_test_connection` probe + `hosts_bootstrap_install` install flow + a **"Reinstall agent on host"** card (`hosts_reinstall_remote` — force-reinstalls `codemux-remote` + restarts its pty-daemon regardless of installed version, for the dev case where a branch rebuild keeps the version string identical so the push-time upgrade skips; reuses the `force_reinstall_remote_binary` helper shared with the push-time `ensure_remote_binary_current`), full `codemux-remote` server binary (`[[bin]] codemux_remote.rs`), and SSH transport (`ssh::probe`/`bootstrap`/`tunnel`/`tunnel_supervisor`/`push`/`registry`) so a workspace can be pushed to a user-owned SSH host. Push synchronizes the worktree, spawns the remote daemon, attaches the local UI through an SSH-forwarded socket, shows a per-pane **migration overlay** ("Switching to <host>…" on push / "Returning to this device…" on pull-back, via the transient `Migrating` lifecycle state) instead of frozen scrollback while PTYs respawn, and **syncs the Claude conversation** (per-project JSONL rsync) **and the OpenCode conversation** (issue #16 — `opencode export`/`import` of the workspace's session via `ssh::opencode_db_sync`, so `opencode --session <id>` continues it on the host without clobbering the host's other OpenCode sessions; see `docs/features/opencode-conversation-sync.md`) across local/remote ends. `WorkspaceSnapshot.host_id` + shared `<DevicePicker>` pill wires the host selection into the new-workspace dialog. **Background `hosts_upgrade` poller** (`hosts_upgrade.rs`) walks every registered host ~5 s after app start and re-bootstraps any whose `codemux-remote` version differs from the bundled binary — users never see the upgrade. Since `v0.7.9` it probes the daemon's `live_terminals` first and **defers** the unit restart while host agents are running, so an auto-upgrade never silently kills host-side work. **Background `hosts_inventory` poller** (`hosts_inventory.rs`) walks every configured host every ~60 s, runs `codemux-remote workspace list` over SSH, and reconciles the host's workspace registry into `workspaces_sync` so host-created workspaces auto-publish to the user's cloud registry without an explicit push (see "Workspaces overview & cross-device sync"). SCP was replaced by an `ssh-cat` pipeline to work around OpenSSH 9+ tilde-expansion. `ssh::probe` falls back to `~/.local/bin/codemux-remote` when the PATH lookup fails so non-interactive SSH on Arch/Ubuntu/Fedora (which doesn't source `~/.profile`) still finds the binary.
- **MCP-on-remote (headless Codemux daemon)**: `codemux-remote serve` runs an axum HTTP server on loopback at a port recorded in `<state-dir>/manifest.json` (mode `0600`, includes a 32-byte bearer secret + `host_id` + `owner_id` reserved for a future relay). `codemux-remote mcp` is a stdio JSON-RPC MCP bridge that reads the manifest and forwards `tools/call` to the daemon over HTTP. The headless tool catalog (separate from the desktop's 52) is **12 tools**: `workspace_{create,list,info,update,close}`, `worktree_create` (added `v0.7.5`), `terminal_{spawn,write,read,list,close}`, `app_status`. Self-contained module at `src-tauri/src/remote/` (`manifest.rs`, `auth.rs`, `identity.rs`, `workspace.rs`, `pty.rs`, `server.rs`, `mcp.rs`, `mcp_register.rs`, `tools/mod.rs`, `git.rs`). `Identity` enum reserves a `Cloud { user_id, org_id, role }` variant for a future paid-tier relay without changing handler signatures. Push auto-provisioning: `ssh::bootstrap::provision_serve` installs the systemd user unit + `loginctl enable-linger`; `ssh::push::push_workspace` drops a workspace-scoped `.mcp.json`; `ssh::bootstrap::register_workspace_on_remote` calls `codemux-remote workspace register` so the pushed workspace shows in `workspace_list` from any agent on the host. On every `serve` startup, `mcp_register.rs` idempotently inserts a `codemux` MCP entry into `~/.claude.json` / `~/.codex/config.toml` / `~/.cursor/mcp.json` so user-level (not just per-workspace) agent sessions also discover Codemux as an MCP server. Desktop-side Step 1 (extract `codemux_core`), Step 5 (pull-workspace UI), Step 6 (`--host` CLI flag), and Step 9 (migrate desktop transport to HTTP+manifest) are explicitly deferred — the headless daemon ships with its own self-contained registry and tools instead of forcing the Tauri-coupled extraction first. See `docs/plans/mcp-on-remote.md`.

### Automations
- **Scheduled agent runs**: a named prompt + agent + RFC 5545 recurrence that fires on a user-chosen host. Each fire creates an isolated git worktree, runs the agent headlessly (`claude --print` / `codex exec`), and records a real `succeeded` / `failed` / `skipped_offline` / `skipped_busy` terminal status. Same-automation overlap is serialised; a per-minute `fire_key` keeps a double tick idempotent.
- **Automations view**: a first-class destination opened from the left sidebar (under "New agent", above the project list) — list + detail pane for create / edit / pause / resume / delete, a frequency/time/weekday schedule builder with a raw RFC 5545 escape hatch, per-automation run history, and a per-row health dot driven by the last run.
- **Account sync**: `automations_sync` replicates the registry through the live `/api/automations` endpoints with the same dirty-flag / tombstone model as `hosts_sync`, so every signed-in device sees the same list; `automation_runs` stay per-device.
- **Host routing**: the desktop scheduler runs only `host_id IS NULL` automations; `codemux-remote scheduler` — a systemd user service provisioned at host bootstrap — runs host-targeted ones on an always-on machine. A stuck-run reconciler fails crashed runs at scheduler startup so a dead run can't pin its automation in `skipped_busy`.
- **GitHub backbone**: a remote host obtains the project repo by cloning / fetching its git remote with the host's own credentials (no token injected); a per-repo `git ls-remote` preflight flags an unreachable repo at setup, not at the first fire.
- Surface: seven `automations_*` Tauri commands + `automations_check_repo_access`, and eight `automation_*` MCP / control-socket tools. See `docs/features/automations.md`.

### Performance
- High-frequency app-state emits coalesced into 16 ms windows — with two documented bypasses: an `app-state-delta` applies immediately (flushing any pending snapshot first so the debounce can never reorder the stream), and the snapshot confirming an optimistic activation skips the window so streaming churn can't starve the selection
- Workspace selection is **optimistic**: a pending active id is written synchronously in the click's own task and Rust is invoked after, so the highlight never waits on the round trip. All activation surfaces share one helper (`activateWorkspaceInteraction`) that owns the rollback and its 5 s backstop
- Agent Chat hydrates by **durable row-id cursor** (`agent_chat_list_messages_after` + `lastPersistedEventId`), so a warm unchanged revisit does no full-history work; tool results over 32 KiB arrive as metadata stubs with bodies fetched on expand, and warm thread slices are evicted on a byte-weighted LRU
- Terminal teardown (scrollback serialize + dispose) is **deferred past the incoming pane's paint**, bounded at two parked jobs and flushed before a same-session remount
- Background loops are change-gated and jittered, so an idle fleet emits nothing: a stride-planned git sweep (active workspace every tick, the rest on a 6-tick stride, per-cwd dedupe), `git fetch` capped at 2 concurrent, hidden-window-gated port polling, and a cheap resource-metrics summary while the monitor is closed
- Three high-frequency state domains (`workspace_git`, `detected_ports`, `pane_status`) ship as ordered `app-state-delta` messages sharing one lock-stamped revision counter with the full snapshot, with a jittered 60–70 s revision heartbeat and frontend gap detection behind them — one workspace's git result re-renders one sidebar card, not the list
- Workspace-switch latency is attributable end to end when `localStorage["codemux:perf-trace"]` is on (`src/lib/perf/interaction-trace.ts`, plus `[codemux::perf::*]` backend section timings); `src/dev/stress-fixture.ts` scales the dev seed to the audited real profile so the numbers mean something
- `transition-all` scoped to actually-transitioning properties
- Markdown view + workspace-tied components no longer re-render on every backend tick
- Workspace-switch mount-time IPC roundtrips cut; IPC thread unblocked
- Editor file read + language module import parallelised
- Worktree-include listener no longer re-attaches every backend tick
- `ensure-draft-when-empty` effect uses a primitive fingerprint
- Chat transcript rows + file-tree nodes memoised to skip per-token re-renders
- Blocking `workspace.rs` + `files.rs` commands moved off the GTK main thread onto `spawn_blocking` (matching the earlier git/gh shell-out migration), so directory listing / ignore checks / workspace ops can't stall IPC
- Agent-chat runtime events (incl. the `content_delta` token stream) arrive per-thread over Tauri Channels instead of the app-wide event bus
- Chat transcript uses real bounded-DOM virtualization with **LegendList** (dynamic height measurement, 800px draw buffer, visible-position preservation, and tail following) — a 5,000-message session does not mount 5,000 rows
- Linux WebKitGTK runs with **accelerated compositing** again: `configure_renderer_env` (`src-tauri/src/webview_tuning.rs`) defaults to `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1`, which keeps GPU compositing (and WebKit's threaded scrolling) while forcing the compositor buffer handoff onto shared memory — sidestepping both the dual-GPU Wayland "Error 71" crash and the GBM/EGL-display failure that the old `WEBKIT_DISABLE_DMABUF_RENDERER` + `WEBKIT_DISABLE_COMPOSITING_MODE` pair worked around at the cost of process-wide CPU rendering (~56 ms → ~16 ms per scroll frame). A small on-disk **crash sentinel** counts startups that never reach page-load and sticks the process back on the legacy CPU flags after repeated failures; explicit user env wins outright. The `tauri:dev` scripts no longer hardcode `WEBKIT_DISABLE_DMABUF_RENDERER=1` (renderer selection belongs to the Rust code), and renderer vars **inherited from a parent Codemux terminal** are scrubbed before the override check so a child process is not silently pinned to CPU rendering; `CODEMUX_WEBKIT_COMPAT=1` is the explicit opt-in to the legacy renderer
- WebKit **smooth (animated) wheel scrolling is off by default** on Linux — its 200 ms eased retarget restarts on every high-resolution wheel event, so a fast flick travelled less than a slow one (WebKit bug 258926). Settings → Appearance → Scrolling exposes a Linux-only toggle to turn it back on (machine-local `appearance.smooth_scrolling`, pushed to every webview via the `set_smooth_scrolling` command and re-applied at boot)
- Chat transcript **edge-fade mask re-enabled on Linux** — it was gated off while the webview was CPU-rendered; measured free (~16 ms frames with and without) now that compositing is back, so the design intent applies on every platform. `localStorage["codemux:transcript-fade"] = "on" | "off"` remains as the override
- xterm.js WebGL renderer offloads glyph rasterization to the GPU where a hardware-GL probe verifies it's a win (software-rendered WebGL + Linux WebKitGTK fall back to the DOM renderer to avoid the `v0.9.0` input-lag regression; DOM fallback also preserved on context loss / missing WebGL2)

## Partial / Being Hardened

- **Browser pane**: screenshot-driven, functional but lower fidelity than a native embedded webview
- **Changes-panel git surface — a cluster of features regressed out of the UI.** The refined-minimal UI pass (`92965c9`, "slim Changes panel + ADE-native right sidebar") removed a set of entry points while leaving every backend, Tauri command, `src/tauri/commands.ts` wrapper, and settings row registered and intact. Nothing in `src/components/` now calls: `mergeBranch` ("Merge [base] into current"), `mergeIntoBase` ("Merge into [base]" temp-branch flow), `resolveConflictOurs` / `resolveConflictTheirs` (per-file Ours/Theirs), `gitLogEntries` (Recent Commits section), or the "Resolve with AI" resolver entry point. The "Against base" compare section and the Alt+Click inline diff are gone too. The panel today is: three file groups, commit bar, push/pull/fetch/amend/undo/stash, and a merge-in-progress banner with Abort/Continue. Treated as an **unintended regression**, not a deliberate slimming. See `docs/features/changes-panel.md` § "Removed by the slim-panel pass".
- **AI merge resolver**: **currently unreachable from the UI**, as a consequence of the above. The Rust backend (`ai.rs`, `git.rs`), all five Tauri commands, their wrappers, the `src/stores/ai-merge-store.ts` state machine (now with **zero importers**), and the Settings → Git → "Merge Conflict Resolver" config row are all still present and registered — but both entry points that started a resolution ("Resolve with AI" in the PR panel, the Changes-panel conflict action) are gone. Re-wiring an entry point is the outstanding work; the hardening fixes below (close stdin + kill child on timeout, skip-permissions flags, blocking-pool offload) are intact underneath it.
- **Project-level sidebar surfaces regressed out of the UI by the workspace inbox (PR #198).** `sidebar-project-group.tsx` is unmounted (zero non-test importers), and it was the sole entry point for **Archive Project** and for **project avatar image/color customization**. **Avatar customization is fixed**: a `Project "<name>"` submenu now hangs off every workspace's right-click menu in the inbox — all three row shapes (active card, settled row, snoozed row) — reusing the same `ProjectImageDialog` and 12-color palette, with writes routed through the shared `src/stores/project-appearance-store.ts` so all of a project's appearance-store avatars repaint at once (the needs-you strip is the one exception: it reads the color key directly and never shows a custom image). **Archive Project is still unreachable** — the remaining half of this regression. See `docs/features/workspace-archive.md`, `docs/features/project-avatars.md`, `docs/features/sidebar.md`.
- **Review tab**: stripped to a resting layout (header + checks + read-only threads). The review composer, merge controls, and deployments were removed **deliberately** (a comment in `review-panel.tsx` records the reasoning); their backends are retained for a possible re-wire. Distinct from the Changes-panel regression above.
- **Browser automation depth**: DOM commands, coordinate commands, OS-level input, wait conditions, JS evaluation, CSS style inspection — all working; toolbar back/forward/reload still need focused validation

## Known Constraints

- Notification click-to-focus on Wayland and mako still needs deeper D-Bus or native handling
- Control socket is local-user only and currently unauthenticated
- Agent Chat GUI is **on by default** (one-time `agent_chat_promoted` migration upgrades pre-promotion installs); opt out back to the CLI view via Settings → Personal → Interface
- Memory drawer UI is still backend + CLI only (no frontend drawer/panel yet)
- File editor: no LSP integration, no multi-cursor, no rename/delete from editor
- Context menus on pane headers are not yet implemented (workspace rows, section groups, tabs, changes panel rows, and sidebar ports section already have them)
- Browser automation uses `agent-browser` v0.24.0 (pure Rust binary, direct CDP). The legacy Playwright/Node.js path and the unused `BrowserManager` Rust CDP implementation are gone.
- Feature docs exist for all major subsystems (see `docs/INDEX.md`)

## Windows Support

Windows support shipped in `v0.1.20` and `v0.1.21` and has been hardened progressively through every subsequent release. The current `v0.17.0` release workflow publishes an NSIS `.exe` alongside Linux artifacts and merges both platforms into the shared `latest.json`; in-app auto-update on Windows was fixed in `v0.5.1`. The MCP-on-remote `codemux-remote serve` daemon is Unix-only by design — the Windows build of that binary is a no-op stub that prints the rationale and exits.

What's in place:

- `cfg`-gates cover every Linux-specific code path — the app compiles on `x86_64-pc-windows-msvc` without unsafe `unix` stubs
- Control socket → named pipe (`\\.\pipe\codemux-{username}`) via `tokio::net::windows::named_pipe`. Client now retries on `ERROR_PIPE_BUSY`.
- Port detection via `netstat -ano` parser (cross-platform pure function, unit-tested on Linux CI) with a Windows system-process name filter
- Agent-browser port reclamation via `netstat -ano` + `taskkill` with exact-port matching; auto-detect installed Chromium (Edge / Chrome / Brave / Chromium)
- `portable-pty` is **upstream `0.8.1`** — the `Zeus-Deus/portable-pty@codemux-0.8.1-no-window` fork was reverted in `06c0301` because `CREATE_NO_WINDOW` broke ConPTY pipe IO (blank panes). The `cmd.exe` console flash is instead suppressed by an `AllocConsole` + `SW_HIDE` on the parent process, release-builds-only
- **PowerShell is the default Windows shell** (`pwsh` → `powershell` → `COMSPEC` → literal `"cmd.exe"`). Agent context injection uses PowerShell `$env:VAR` syntax; preset commands terminate with `\r`. Gemini path writes its system-prompt temp file via PowerShell `Set-Content -NoNewline`.
- Editor detection uses `which::which()` + `%LOCALAPPDATA%\Programs` / `%ProgramFiles%` fallbacks for VS Code, Cursor, VSCodium, Zed; JetBrains stays PATH-only
- `<WindowChrome />` extracted so login, empty-state, settings, and new-project screens have minimize/maximize/close buttons (Codemux runs with `decorations: false`)
- Scrollback flush waits 10s on Windows (3s elsewhere); Windows-only `scrollback::flush_cache_to_disk` backend backstop catches anything the frontend can't persist before timeout
- `release.yml` builds on `[ubuntu-22.04, windows-latest]` with `fail-fast: false`; tauri-action merges both platforms into a single `latest.json`
- NSIS installer produced on Windows CI (`--bundles nsis` to skip MSI which needs WiX)
- Claude Code hooks register and execute on Windows
- Tier-3 OS-level input injection via Win32 `SendInput`
- Path normalization + four latent windows portability issues fixed

Still gated before a polished Windows v1:

- Windows Authenticode code signing (SmartScreen friction expected on unsigned first-install; deferred behind a cert budget decision)
- Full PTY lifecycle / worktree / agent-spawn integration tests on a live Windows runner

See `docs/plans/windows-support.md` for the complete checklist.

## React Frontend Status

The frontend is React + Tailwind v4 + shadcn + Vite. The Rust backend is unchanged. The old Svelte frontend has been removed.

### Working

- App shell: shadcn Sidebar rendering the flat workspace inbox (search + project filter + newest-first workspace cards + collapsible "Snoozed" and paginated "Settled" shelves; PR #198 replaced the collapsible per-project tree), tab bar, right panel
- Workspace list from real Tauri backend data (zustand + app-state-changed events, coalesced into 16ms windows)
- Terminal panes with xterm.js WebGL (hardware-GL gated) + DOM fallback + PTY via Tauri Channel, persistent across workspace switch
- Pane splits (horizontal/vertical) with CSS Grid, resize handles, drag-to-swap
- Right panel as a **pane deck**: **one row** of panel chrome — closable
  icon tabs plus a `+` menu on the left and the active pane's own actions on
  the right, rendered *in the window's 40px titlebar band* in GUI chrome —
  over a flush pane body and a 26px status foot (active pane's status + the thread's running
  token total). The separate 32px breadcrumb/pane bar that used to sit
  between the tabs and the body is gone: it repeated the workspace name and
  the pane name, and stacked a third band above the first line of content.
  Its buttons moved into the tab row; the text it carried (browser URL, diff
  file) moved into the status foot. Panes are declared in
  `src/components/layout/right-panel/pane-registry.ts` — Files, Changes,
  Diff, Review, plus conditional Tasks, Subagents and Orchestration — and a
  file opens as its own `doc:` pane. Open order, active pane and
  user-dismissed panes persist per workspace in `ui-store`. The `+` menu also
  routes Browser / Terminal to the existing workspace-pane actions and
  "Open file…" to the file-search dialog. Agent Tasks is still a durable,
  thread-focused, read-only provider plan fed by Claude (`TodoWrite` +
  `Task*`), Codex (`turn/plan/updated`), and OpenCode (`todo.updated`); its
  compact composer toggle appears only after a non-empty plan exists
- Agent Chat UI: chat pane, composer (with `+` popup, `@` mention popup, slash command popup, image paste/drop), transcript, mode pill, model picker, session selector, attachment chips, plan proposal block, AskUserQuestion panel, thinking indicator, permission request block, tool-call card with per-tool body rendering, debug-mode banner + exit dialog; runtime events (incl. `content_delta` token stream) arrive per-thread over a Tauri Channel (`attach_agent_chat_output`, issue #75) instead of the global event bus; transcript body uses LegendList bounded-DOM virtualization while preserving the issue #77 stick-to-bottom contract; `SubagentActivityBar` welded inside the composer's top edge while subagents run
- Settings panel (15+ sections including Interface, Sync, Skills, MCP, Permissions)
- Command palette (Ctrl+K) with fuzzy search
- Search: file name search (Ctrl+Shift+P) and content search (Ctrl+Shift+F)
- Browser pane with screenshot-driven rendering and toolbar (reactive `stream_url` reconnect)
- Workspace drag-and-drop reordering in the sidebar tree components (retained in-repo; not wired into the flat inbox that now renders the expanded sidebar)
- Terminal presets bar with quick-launch
- Auth system with GitHub OAuth, email/password, encrypted token storage
- Synced settings (per-user server-synced with offline cache)
- Skills sync UI (Settings → Account → Sync; two-state ready/sign-in dashboard, server-side model, no password forms)
- Semantic theming: shadcn oklch dark mode + custom --success/--danger/--warning tokens
- Tauri bridge: ~310 typed `invoke` wrappers in `src/tauri/commands.ts`, 18 event helpers in `src/tauri/events.ts`, all shared types in `src/tauri/types.ts`

### Remaining Gaps

- Context menus on pane headers (workspace rows, workspace section groups, tabs, changes panel rows, and sidebar ports section already have them)
- Memory drawer UI (backend memory system exists, CLI works, no frontend drawer/panel yet)
- File editor: no LSP integration, no multi-cursor, no rename/delete from editor

## Read This With

- `docs/core/PLAN.md` for build order
- `docs/core/TESTING.md` for verification policy
- `docs/features/*` for subsystem detail
- `docs/plans/windows-support.md` for the cross-platform checklist
