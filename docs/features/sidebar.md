# Left Sidebar

- Purpose: Describe the left sidebar shell — the flat workspace inbox (expanded) and the collapse-to-icon-rail behavior.
- Audience: Anyone working on sidebar layout, navigation, or workspace presentation.
- Authority: Canonical feature-level reality doc for the left sidebar.
- Update when: The sidebar layout, inbox model, park (settle/snooze) lifecycle, collapse model, or rail rendering changes.
- Read next: `docs/reference/SHORTCUTS.md`, `docs/features/notifications.md`

## What This Feature Is

The left sidebar is the primary navigation surface. Expanded, it is a **flat
workspace inbox**: a search affordance + new-agent button, a project-filter
dropdown row, one multi-line card per active workspace (with a "Wrapping up"
tier of winding-down cards below a static divider), a durable pinned-card
block above the normal active list, a collapsible
"Snoozed" shelf of deferred rows, and a collapsible "Settled" shelf of
swept-aside one-line rows. Collapsed, it is a narrow icon rail.
The sidebar surface is **darker than the main pane** (a `--sidebar` override
in the custom-token layer of `globals.css`), so cards sit flat/transparent
at rest and *which workspace you are in* reads as lightness — no accent color
for that. Accent is reserved for the claims lightness can't make: the
needs-you red border, the ember unread dot, the ember ring that marks a row as
checked for a bulk action, and the green "Woke" pill.
The same doctrine runs in the other direction as **background recede**: full
brightness is reserved for the cards that want a human — the workspace you are
in, needs-you, done-review, unread, just-woke, and multi-selected — while a
card whose agent is quietly working, or that is idle and already read, sits at
reduced opacity. Nothing is hidden: hovering or focusing a receded card
restores it in full.

## Current Model

The sidebar uses the shadcn `Sidebar` primitive in **`collapsible="icon"`** mode.
There are exactly two states, toggled by the title-bar button and `Ctrl+B`:

- **Expanded** — the workspace inbox (resizable 180–400px, default **288px** —
  widened from 256 so the card meta line fits).
- **Icon rail** — a 52px (`SIDEBAR_WIDTH_ICON = 3.25rem`) vertical rail of icons.
  The rail is **always visible**; the sidebar never fully disappears.

### The workspace inbox (expanded)

Replaced the nested project tree (project groups, drag-reorder, and the
"Gather on top" LIVE section) with one flat list. The pinned "Needs you"
strip survived the migration: it is re-mounted in the inbox's sticky header
(see below).
A workspace has one base lifecycle — **active** (a card), **settled** (a row on
the Settled shelf), or **snoozed** (a row on the Snoozed shelf). A durable
**workspace pin** is an orthogonal visibility override: while `pinned_at` is
set the workspace renders as a card in the pinned block regardless of its base
lifecycle, and unpinning reveals the still-preserved settled/snoozed state.
The override is against *parking*, not against un-parking: a pinned card's
preserved entry is never destroyed by a settle or snooze, but the settle safety
net and the wake sweep still clear it when the agent goes live or the wake time
elapses (see "The anti-oscillation invariant").
This is distinct from the inbox store's internal `keepActive` auto-settle
override created by Un-settle. "Parking" is the shared verb for settling or
snoozing; parking is visual only — nothing is archived, closed, or deleted.

- **Header** (`sidebar-action-row.tsx` expanded variant): a search box-shaped
  button that opens the command palette (shows the resolved `⌘K` keybind) and
  a neutral-ghost new-agent button (same click/shift-click semantics as
  before). Add repository moved into the filter row; Automations and Workspaces
  moved into the footer nav row (see "Footer" below). The palette indexes every
  workspace record — active **and** settled — so search reaches a workspace
  parked onto the Settled shelf. It matches on name (`title` + `git_branch`)
  by default and switches to locations (`project_root`, `worktree_path`,
  `cwd`) as soon as the query contains a `/`; parked workspaces sort last.
  See `docs/features/command-palette.md` for the full ranking rules.
- **Project filter dropdown** (`sidebar-inbox.tsx`): one 32px sticky row
  (`h-8` trigger + `size-8` add-repo button, matching the action row above) —
  a flex-1 trigger showing the current filter (Folder icon + "All projects",
  or the repo's mini avatar + name) with a rotating chevron, plus the dashed
  `+` add-repo button pinned right (Open project / New project dropdown).
  The panel lists **All projects** + one row per project (avatar, dedup'd
  name, right-aligned **visible-card count** — unsettled and unsnoozed, plus
  pinned workspaces whose preserved base lifecycle is parked, so a wrapping-up
  or pinned card still counts; All shows the total); the current filter's row is highlighted; picking sets the
  filter and closes. The filter applies to **both** the active cards and the
  settled rows and resets settled-tail paging; a filtered-empty list shows
  "Nothing active in `<repo>`". Session-only, and self-healing — if the
  filtered project disappears (its last workspace archived or deleted) the
  filter resets itself to "All projects". (Replaced the earlier horizontal
  filter-chip strip and its wheel-scroll handling.)
- **"Needs you" strip** (`sidebar-needs-you-strip.tsx`): pinned in the same
  sticky block as the project filter, so it stays visible however far the card
  list scrolls. Renders only while ≥1 workspace has status `permission`;
  absent otherwise. Each entry is a **jump-link** (project avatar + blocker
  text + age) that activates the blocked workspace and smooth-scrolls its card
  into view — the card is never moved or removed, so the strip surfaces
  blocked work **without reordering the list**. That duplication is what lets
  the active list stay status-blind. Entries are sorted oldest-blocked-first
  by each blocker's permission timestamp (density-store `statusSince`; an
  unseeded timestamp ranks as newest, ties keep stable tree order), so the
  longest-waiting agent is on top — tree order alone wouldn't deliver that,
  since the list runs newest-first and a block can land on any card at any
  time. Capped at **4** rows with a `+N more below` line; the sort means the
  cap can only ever hide the newest blockers, and the header count
  (`NEEDS YOU · N`) always reports the true total. Scoped by the active repo
  filter, so a filtered sidebar never points at a workspace the list is
  hiding. The strip applies **no settled/snoozed filter**, and doesn't need
  one: the settle safety net and the snooze hand-raise (below) resurface any
  parked workspace the moment it goes `permission`, so a strip entry's card
  is always genuinely in the active list. It is also the **one surface that
  orders by status** — the list below stays deliberately status-blind, and
  the strip existing is precisely what lets it.
- **Pending workspaces**: workspaces still being created render as their own
  rows **below the top-tier cards and above the "Wrapping up" divider** — a
  spinner "creating" row, or a red `AlertCircle` "failed" row. They are filter-scoped like everything else and suppress the
  "Nothing active" empty state while present.
- **Pinned card block** (`WorkspaceSnapshot.pinned_at`): pinned workspaces render
  before every normal active card, separated by a thin unlabeled hairline. The
  hairline is a *separator*, so it renders only when there is at least one
  non-pinned card, pending row, wrapping-up card, or shelf below it — an
  all-pinned (or empty) list ends cleanly rather than trailing a rule under
  nothing. The block uses the same stored-index ordering as the normal list —
  creation order, not pin-click time — so pinning several workspaces does not
  make each new pin jump ahead of the others. Pin/unpin is idempotent and
  persisted in `layout.json`; archive/unarchive preserves membership **and the
  original `pinned_at` timestamp** (restore writes the archived value verbatim
  rather than re-stamping "now"). A pin suppresses Settle/Snooze actions,
  excludes the workspace from bulk parking and auto-settle, and overrides any
  existing shelf presentation without deleting it. The card eyebrow carries a
  pin glyph and exposes a direct **Unpin** action on hover/focus; every
  workspace context menu begins with **Pin workspace** or **Unpin workspace**.
- **Card order — newest first, and static** (`compareNewestFirst` in
  `sidebar-inbox.tsx`): within the pinned and normal blocks, cards sort by
  **stored snapshot index, descending**.
  The backend has no `created_at`, but it appends new workspaces to the
  snapshot array, so a later stored index is a newer workspace. The order is
  derived from that index *only* — never from status, activity, or
  notification counts — so a row holds its position from the moment it opens
  until the user settles or snoozes it. That is what makes `Alt+1..9` muscle
  memory: **`Alt+1` is the newest workspace**, not the oldest. Status is
  carried by each card's own state cluster (and by the pinned needs-you strip)
  instead. Honest trade-off: newest-first means every *creation* shifts the
  existing cards (and their jump digits) down one, and a tier change (a card
  entering or leaving "Wrapping up") moves it across the divider — so
  positions and digit assignments are stable *between* those lifecycle
  events, not absolutely. That is accepted on purpose: the win is that the
  workspace you just created is always at the top, on jump slot 1, rather
  than below every older card.
- **The "Wrapping up" tier** (`isWrappingUp` in `sidebar-inbox.tsx`): the active
  list is partitioned into two tiers — everything else on top, winding-down
  cards under a **static "Wrapping up" divider**. A card is wrapping up when
  **all** of: its PR state normalizes to **`open`** (explicitly *not* `draft` —
  a draft PR is work the author still owns — and not `merged`/`closed`, which
  are completion signals handled by auto-settle); its status is **null**; and
  it is **not unread**. Opening a PR is a *wind-down* signal, not a completion
  one: the card keeps every affordance (Settle, Snooze, context menu, jump
  target, selection) and nothing is hidden or collapsed — it just stops
  occupying the top of the list. The unread condition is the load-bearing one:
  unread means the agent produced output the user has not looked at, and
  demoting that buries exactly what they opened the sidebar to find. The status
  condition is what makes "reopen the PR workspace and send a follow-up" work
  with no extra state — the follow-up makes the agent work, status goes
  non-null, and the card returns to the top tier by itself. Each tier is sorted
  by the same `compareNewestFirst`, so newest-first still holds *within* a
  tier, and membership flips only on durable lifecycle events (a PR opening, an
  agent run the user caused, the user reading the result) — never on transient
  status churn, which is what keeps this consistent with the static-order rule
  above. The divider is a plain label + hairline (`WrappingUpDivider`),
  deliberately **not** the collapsible `ShelfHeader`: a chevron and
  `aria-expanded` would promise these rows can be folded away, and hiding
  live-but-nearly-done work is the failure mode the tier exists to avoid. No
  qualifying card means no divider at all.
- **Workspace cards** (`sidebar-inbox-card.tsx`): each active workspace is a
  card — repo avatar + name eyebrow; work title (linked-issue title while an
  agent is live, worktree name when idle) + issue chip; a red blocker line
  (needs-you only); and a mono meta line. The meta line is **two columns, not
  one flow**: the git-local facts (branch · `↑ahead` · `+/−` diff) flow from the
  left, then a `flex-1` spacer pins the rest to the right — PR chip · provider
  logos · remote cloud icon · notification badge. The PR chip is **the same
  borderless badge settled rows use**: a state-colored `PrStatusIcon` + `#n`
  (open green, merged violet, closed red, draft muted), no border or fill, with
  a hover tint only when there is a `pr_url` to open (dimmed and `disabled`
  otherwise, so the chip itself swallows the click instead of passing it to the
  card). Active and settled therefore render one PR the same way, so a card
  settling no longer looks like the badge changed. The spacer sits *before* the
  PR chip deliberately: with it after, the chip began wherever the branch name
  happened to end, so chips landed at a different x on every card and the column
  read as ragged. Right-aligning also makes the chip's variable width
  (`#7` vs `#1234`) harmless. The
  trailing indicator cluster owns the far-right column and reserves a
  `min-w-[15px]` slot — sized to the *widest* single indicator (the notification
  pill, not the 13px provider logo) so neither a bare card nor a card showing a
  different indicator can shift the chip. The invariant is guarded by the
  "meta line alignment" tests in `sidebar-inbox-card.test.tsx`, which assert DOM
  order around the spacer (jsdom does not lay out, so pixels cannot be asserted).
  The blocker line is a fixed string
  (`permissionBlockerText()` always returns "Waiting for your input" — it does
  not surface the agent's actual question). The right side of the eyebrow shows
  the agent state — Working (configurable `WorkingIndicator`, amber text) /
  Needs you (pulsing red dot) / Monitoring (steady cyan dot, never animated —
  see `docs/features/monitoring-status.md`) / Done · review (green ✓) /
  elapsed since the workspace last settled into review — and swaps to a
  **"✓ Settle"** + **"Snooze"** action pair on hover or focus for a normal
  card, or a direct **"Unpin"** action for a pinned card (CSS-only swap, plus
  an action-visible
  state while the Snooze dropdown is open, since Radix portals the menu out of
  the card and would otherwise lose its own trigger). The selected card
  gets a neutral border + clearly lighter fill (selection is lightness, not
  accent color — the sidebar surface is darker than the main pane, so cards
  sit flat/transparent at rest and lift on hover); needs-you cards a
  red-tinted border; multi-selected cards an **ember ring** layered over
  whatever the card already is. A card that is **none** of those and whose
  agent is working, monitoring, or absent (idle) — and that has been read and
  has no "Woke" pill — **recedes** to `opacity-70`, restored to full by hover or
  `focus-within` (the dim is suppressed outright while the card's Snooze menu
  is pinned open, since Radix portals that menu out of the card and neither
  restore would hold). The dim rides the card's own container, not the settle
  animation wrapper, which already owns an opacity axis. It deliberately
  includes "Wrapping up" cards: an open PR on an idle, read card is exactly the
  work that is nobody's problem this minute. The status label keeps its hue
  inside a dimmed card — the row is quieter, not recolored. Click activates; the right-click context menu
  is the same `WorkspaceContextMenuItems` (rename, editors, move-to-host,
  archive, delete) shared with the old row, including the delete/push-confirm
  dialogs.
- **Hover details** (`workspace-hover-card.tsx`): resting the pointer on any
  workspace surface — an active card, a settled or snoozed one-line row, or a
  collapsed rail avatar — opens a shared read-only `HoverCard` to the right
  (350ms open, 120ms close, `side="right" align="start"`, 290px). It exists
  because every sidebar surface is lossy: the card truncates its title and
  drops `git_behind` / `git_changed_files` entirely, the settled and snoozed
  rows show only a title, and the rail shows nothing but an avatar. Contents, all conditional
  except the header and Location: repo eyebrow + provider marks + agent state
  (with client-derived elapsed), the **full** untruncated title, the linked
  issue's number + title, then label/value rows — Branch, Uncommitted `+A −D`,
  Changed files, Ahead, Behind (or a single "Working tree · clean" row when
  there is nothing to report), Pull request, Issue, Port(s) (capped at 3 with
  a `+N` overflow), Location (`This device` / host name / `host · in place`),
  Notifications muted — and finally the real path on disk (`worktree_path ??
  remote_cwd ?? cwd`, `$HOME` collapsed to `~`, wrapped not truncated).
  **Parked rows report real agent state, not "Idle."** A settled or snoozed
  workspace can still be running — a review-state agent stays settled — so
  `SettledRow` / `SnoozeRow` take a `status` prop that their call sites derive
  live via `getWorkspaceStatus(workspace.surfaces, paneStatuses)` rather than
  hard-coding idle into the parked shapes.
  It reuses the `DetailRow` label/value shape from the shared status cluster
  (`WorkspaceStatusCluster`), and needs **no new Tauri command** — every field
  is already on `WorkspaceSnapshot` or in `appState.detected_ports`. The card
  keeps pointer events enabled so the path and branch can be selected and
  copied (Radix holds it open while the cursor crosses into it), and its
  body is a separate component so Radix's unmount-when-closed keeps a sidebar
  of N workspaces from subscribing N× to the ports/hosts stores at rest. It
  mounts *inside* `WorkspaceInboxMenu` on a different node than
  `ContextMenuTrigger`, so the two `asChild` triggers never collide.
- **Project section of the workspace menu** (`project-appearance-menu.tsx`): all
  three row shapes' right-click menus (active card, settled row, snoozed row)
  carry a `Project "<name>"` submenu between the workspace actions and the
  device actions — an image entry plus the 12-color accent palette, applying to
  the whole project the row belongs to. This is the re-homed project-avatar
  customization that lost its entry point when the project tree was unmounted.
  `docs/features/project-avatars.md` is the canonical doc for the mechanism
  (store, persistence keys, which surfaces repaint).
- **Settle / un-settle** (`sidebar-inbox-store.ts`): Settle collapses the card
  (~200ms height/opacity), then moves it onto the "Settled" shelf as a compact
  one-line row (repo avatar · title · state-colored PR icon + `#number` when
  linked · elapsed-since-work-ended). The PR badge opens the pull request
  directly and remains visible while the row's Un-settle affordance appears.
  With no `pr_url` the badge renders disabled and dimmed, still labeled
  `PR #n — <state>`.
  The shelf takes the background-recede doctrine one step further than the
  cards do: a settled row is history, so at rest the whole row goes grey —
  desaturated repo avatar, faint title, and a PR badge that gives up its state
  color (`prStatusSettledHoverClass` defers each state's color to a
  `group-hover/settled:` variant; the icon carries `text-current` so number and
  glyph light up together). Hover or keyboard focus restores all three at once,
  so the tail stays scannable when you are hunting. The workspace you are
  currently in and any row ticked for a bulk action are excluded — they keep
  full prominence, the same exclusions the receded cards use.
  A settled row is itself a button — click or
  Enter/Space activates that workspace without un-settling it. Row activation is
  scoped to the row node by `isRowActivationKey` (`Enter`/`Space` **and**
  `e.target === e.currentTarget`), so pressing Enter/Space on an inner control —
  the PR badge, Un-settle, Wake now — runs only that control's action instead of
  also activating the workspace. Snoozed rows and active inbox cards share the
  same guard (`sidebar-row-activation.ts`), so Enter/Space on a card's PR chip
  opens the PR without also yanking the main pane onto the workspace. Because it is
  DOM-identity based, any inner interactive element added later is exempt
  automatically. Hover/focus
  reveals **Un-settle**, which reverses it (the returning card eases back in
  via the shared `rise-in` keyframe). Settling is **visual only** — nothing is
  archived, closed, or deleted; a settle still mid-animation is flushed to the
  store on unmount, so collapsing the sidebar mid-gesture doesn't drop it. The
  shelf persists via UI-state key `sidebar.inbox.settled`, pruned when a
  workspace vanishes (both the inbox and the collapsed rail run the prune, so a
  session spent entirely collapsed still trims the blob). The list is flat —
  repo identity is carried by each row's avatar, not by project grouping.
  All three row shapes (card, snoozed row, settled row) share the full workspace
  right-click menu via `workspace-inbox-menu.tsx`. It begins with Pin/Unpin;
  its lifecycle block is ordered so the entries that bring work *back* come
  first — Un-settle, Wake now, then Settle, "Snooze until…", Mark unread — since a user who right-clicks
  a hidden row is usually there to undo the hiding. Every shape keeps
  rename / archive / delete / move-to-host below that. Settle and Snooze are
  absent while pinned, while Mark unread remains available.
  Because a parked (settled or snoozed) workspace is still fully open, it also
  stays a valid target in agent chat's "Run in" location picker, which reads
  this same store to split its list into **Active** and **Settled · still
  open** sections — both parked lifecycles fold into the parked side (see
  `docs/features/agent-chat.md` → "Thread Scope"). The picker never writes the
  store: selecting a parked project doesn't un-settle or wake it, because the
  safety nets here already resurface it once its agent runs.
- **Settled shelf ordering — by when work ENDED**: a settled entry records both
  `at` (when the sweep happened) and, when the caller knows it, `workEndedAt`
  (the workspace's backend `last_active_at` at settle time).
  `resolveSettledTimestamp(entry)` returns `workEndedAt ?? at` and is the *only*
  key used for both the sort and the elapsed label, so a row can never sit in
  one place while claiming a different time — manually sweeping aside a
  month-old workspace files it under last month, not at the top of history.
- **Snooze** (`sidebar-snooze.ts` + the Snoozed shelf): a second parking
  lifecycle beside Settle. A card's **Snooze** button (and the "Snooze until…"
  context submenu) offers wake-time presets — **In 1 hour / This evening
  (18:00) / Tomorrow (09:00) / Next week (next Monday 09:00)**. "This evening"
  is conditional: it is dropped unless it is at least an hour away, so it can
  never resolve to a time in the past or fire while the menu is still open.
  "Next week" is conditional the same way, against a different neighbour: on a
  Sunday the coming Monday *is* tomorrow, so the preset is dropped unless it
  lands at least a calendar day past "Tomorrow" — two entries resolving to one
  wake instant would read as a menu bug.
  Presets are built from **local date components**, not by adding
  `86_400_000ms`, so a "Tomorrow" taken the night before a DST change still
  wakes at 09:00 local. `computeSnoozePresets(now)` is pure and clock-free —
  the caller passes `now` — so the menu and the wake sweep can never disagree
  about an instant. Each preset also carries a **`whenLabel`**
  (`formatWakeLabel`): the wall-clock instant it actually fires, rendered muted
  and mono beside the relative label — `18:00` for later today,
  `Tomorrow 09:00` for the next day, `Mon 09:00` beyond that, all through
  `Intl`/`toLocale*` so a 12-hour locale reads "6:00 PM". Without it "Next
  week" is a deferral whose end the user has to guess, and "In 1 hour" taken
  at 23:30 doesn't say it lands on another day. The day prefix is decided by
  comparing **local day starts**, so the two 23/25-hour days a year still read
  as "Tomorrow".
- **Presets are resolved when a menu opens, never when the list renders.** The
  three owners each call `computeSnoozePresets(Date.now())` for themselves: the
  card's Snooze dropdown in its `onOpenChange`, the shared context menu's
  `SnoozeUntilSubmenu` (its own component precisely so Radix mounting the menu
  content *is* the resolve), and the bulk menu's Snooze submenu in its
  `onOpenChange`. This replaced one array recomputed on the inbox's coarse
  clock and passed down to every row, which made "In 1 hour" mean "an hour from
  up to a tick ago" by the time it was clicked and pushed a fresh array
  identity through the whole list on every tick.
- **Snoozed shelf**: sits above Settled, **collapsed by default** (its rows are
  by definition work the user said they did not want to see; re-showing them
  each launch would undo the gesture). Rows are ordered soonest-wake-first and
  deliberately share the settled row's one-line silhouette — both are "parked".
  Each shows **time-until-wake** (`formatTimeUntil`: `45s` / `12m` / `3h20m` /
  `2d4h`, and `now` once due, never a negative duration) plus a hover-revealed
  **Wake now**. Snoozes persist in the same blob (`snoozed: [{id, at, until}]`).
- **Wake paths**: three, and only three. (1) The **wake sweep** on the coarse
  ~30s clock wakes anything whose `until` has passed. (2) A **precise timer**
  armed at the soonest `until` covers the gap the coarse clock would leave (an
  "In 1 hour" returning at 1h00m29s reads as a broken promise); its delay is
  passed through `clampTimerDelay`, because `setTimeout` stores the delay as a
  signed 32-bit int and a "Next week" snooze would otherwise overflow negative
  and fire *immediately* — clamping re-arms at the ceiling and the effect
  schedules the remainder on its next pass. A nonce re-arms the timer after
  each fire so a queue of wakes is walked one boundary at a time. (3) The
  **hand-raise**: a snoozed workspace whose agent goes `working` or
  `permission` wakes at once, wake time or not — a snooze defers waiting, it
  must never hide an agent blocked on a question. Timer/hand-raise wakes badge
  the returning card with a green **"Woke"** pill (the list order is static, so
  a woken card slots back where it was and nothing about its position says it
  moved); an explicit "Wake now" gets no badge, and the badge clears on visit.
- **Settle safety net**: live work can never be buried. A card whose agent is
  working or blocked ("needs you") offers neither Settle nor Snooze (its state
  cluster stays visible on hover — `isSnoozeable` mirrors the card's `canSettle`
  rather than restating the condition), and a *settled* workspace whose agent
  becomes working/blocked is **auto-un-settled** (persistently, with the
  rise-in ease). Finished ("review") and idle cards park normally and stay
  parked — sweeping completed work aside is the point of the gesture.
- **Backend activity stamps** (`WorkspaceSnapshot.last_active_at` /
  `last_visited_at`, ms epoch, persisted in `layout.json`, both additive serde
  fields that deserialize as `None` on old state):
  - `last_active_at` is stamped by `stamp_workspace_activity` whenever any pane
    in the workspace transitions to a **non-idle** status (working / permission
    / review) — via `set_pane_status`, `set_pane_status_by_session`, and
    `set_pane_status_by_thread` alike, so terminal and Agent Chat panes both
    feed it. Idle is deliberately **not** stamped: going quiet is the absence of
    work. Chat panes stamp below the change-guard, so a token stream that
    re-asserts `Working` doesn't write per token. Also stamped at workspace
    creation.
  - `last_visited_at` is stamped **only by `record_workspace_switch`** — the
    one private helper every path that moves `active_workspace_id` calls:
    `activate_workspace` (sidebar click, palette, keyboard jump, control
    socket) unconditionally, and `activate_terminal_session` /
    `activate_pane` (jump-to-session navigation, pane focus) whenever the
    pane being focused lives in a *different* workspace. A same-workspace
    focus move does not stamp — a glance is not agent work, which is why the
    two stamps stay separate: collapsing them would let merely looking at a
    workspace keep dead work permanently unswept. The helper stamps **both
    edges of a switch** — the workspace being entered *and* the one being
    left. A visit that ends now lasted until now, and stamping only the entry
    edge marked work the user sat and watched finish as unread the moment
    they switched away (the pane writers stamp `last_active_at` even while
    the workspace is focused, so `last_active_at > last_visited_at` came out
    true for the one workspace the user had definitely seen). Re-activating
    the already-open workspace has no outgoing side and stamps only itself.
  - **Boot backfill** (`backfill_workspace_activity`): workspaces persisted
    before the field existed are dated from their checkout's **last git commit**
    (`git log -1 --format=%ct`), else the **directory mtime**, else left `null`.
    It never invents `now` — a fabricated "just active" stamp would exempt
    genuinely stale work from the sweep forever, while `null` reads as "unknown"
    and the frontend declines to sweep on it. Backfill quality therefore
    depends on the workspace being a git repository: a non-git checkout falls
    to the mtime fallback, and a directory's mtime is often close to "now". It only probes workspaces still
    missing a stamp (so it goes empty on the next launch), skips host-backed and
    attach-only workspaces (their paths name directories on another machine),
    and re-checks `is_none()` under the lock before writing, since the git work
    runs unlocked and a live agent may have stamped a real value meanwhile.
    The host-backed skip is why an **adopted** workspace is dated one launch
    after its pull rather than never: adoption clears `host_id` as soon as the
    files are local, so the next boot pass sees an ordinary unstamped local
    checkout and reads the git history that came across with it.
  - This replaced a client-side `Date.now()` first-seen baseline that reset
    every workspace's idle clock on each app update.
- **`effectiveActivityAt(backendAt, clientAt)`**: the idle sweep measures
  against `last_active_at`, falling back to the client `activity` map only when
  the backend has no stamp at all. **The client map must never win.** Installs
  predating `last_active_at` wrote a synthetic `Date.now()` baseline into that
  map for every workspace they had; those stamps are indistinguishable from real
  activity, so honouring them would make a machine full of month-old work look
  brand new for a full idle window after every update — the exact bug the
  backend field exists to kill. Preferring the backend retires the polluted
  state without a migration pass.
- **Auto-settle** — the Settled shelf fills itself using PR-completion
  semantics. A workspace whose PR is **merged or closed** settles
  immediately once it is neither working nor blocked; no activity stamp or
  extra idle grace is required — provided that PR is the **checked-out
  branch's** and not a badge-only side-branch association (see "Side-branch PR
  badges" below). A completed **review** status is settleable —
  it says the run finished, not that work is still executing. An **open** PR
  never settles a card by itself; it demotes it into the "Wrapping up" tier
  described above. Work without a finished PR auto-settles when:
  - it has gone untouched past the user's idle window
    (Settings → Appearance → Sidebar → "Auto-settle idle work":
    Off / 1d / 3d / 7d / 14d, `sidebar.auto_settle_days`, default 3d).

  Only the inactivity rule requires a known stamp — `undefined` never sweeps
  unknown-idle work. The merge/close rule may also settle the currently-open
  workspace: settlement is classification, not navigation, and the forced-row
  visibility rule keeps its highlighted row reachable while the main surface
  stays open. The sweep runs no leaving animation and performs **no forward
  navigation**, and it settles with
  `workEndedAt = last_active_at` so the row files under when work ended.
  **Un-settling sets a keep-active pin** that suppresses auto-settle until the
  agent shows a new non-idle status edge. Repeatedly observing an existing
  completed `review` status does not clear the pin. The persisted UI-state value is
  `{settled, snoozed, keepActive, activity}`, with transparent migration from
  the older bare-array and pre-snooze object shapes; the key name
  (`sidebar.inbox.settled`) predates snooze and is kept so existing installs
  don't lose their shelf.
- **PR association is branch/repository based, not commit-SHA based.** The Rust
  refresh path explicitly lists all PR states for the checked-out branch via
  `gh pr list --head ... --state all`. This is important after review: GitHub's
  final PR head can contain commits that the still-open local worktree has not
  fetched, but that does not make it a different work item. The query always
  passes the **bare** branch name: `gh pr list --head owner:branch` is accepted
  but matches nothing (verified on gh 2.96.0), so a fork-tracking workspace
  asked that way would come back empty and — an empty result being
  authoritative — lose its badge every poll tick. Fork disambiguation happens
  client-side instead, by matching the row's `headRepositoryOwner` against the
  owner the branch tracks. If a branch name was reused, the newest open/draft PR
  wins over historical matches; otherwise the newest merged/closed match wins,
  with candidates ordered newest-first so the answer never depends on the order
  gh returned. Historical matches are suppressed on the default branch, where
  they usually represent reverse-merge history rather than the workspace's work.
  A successful empty query clears stale PR metadata, while an *unanswerable*
  lookup leaves the last known badge untouched — a failed or timed-out `gh`
  call, and also a detached HEAD (mid-rebase, mid-bisect), which has no branch
  to answer about and so must not be read as "no PR". All three consumers route
  through one `branch_pr_outcome` helper (`Write` / `Clear` / `Preserve`) so the
  matrix cannot drift between the manual refresh command and the two pollers.
  These rules are what let a real `MERGED`/`CLOSED` transition reach auto-settle
  reliably.
- **Side-branch PR badges (badge only).** Strict current-branch association has
  one visible blind spot: an agent that runs `git checkout -b side-branch`,
  commits, pushes, opens a PR, and checks the worktree back leaves a workspace
  that plainly produced a pull request and no badge to show for it. So when the
  checked-out branch resolves to *no* PR, `github::get_workspace_pr` falls back
  to branches the worktree checked out recently: it parses `checkout: moving
  from X to Y` records out of the last **50** HEAD reflog entries (that reflog
  is per-worktree, so it describes this checkout's own history), takes up to
  **5** distinct branch names newest-first, and resolves them through the same
  `select_branch_pr` policy. Both sides of each record are read (`Y` then `X`)
  so a branch whose arrival scrolled out of the window is still found; the
  current branch, the default branch, and detached-HEAD SHAs are excluded.
  Recency decides between candidates — the first one with a qualifying PR wins
  outright, because "what this workspace was just doing" is the question being
  answered. The fallback never runs on the repository **default branch**, where
  "recently checked out" describes ordinary branch hopping rather than this
  checkout's work. A failed fallback collapses to "no PR" rather than
  `Preserve`: the current branch already answered authoritatively, and a
  missing bonus badge must not resurrect an old one.
- **The fallback badges open PRs only**, unlike the current-branch path, which
  keeps showing merged and closed state. The case it exists for — "an agent
  just pushed a PR from a side branch" — is always an open PR, while admitting
  history would let any branch that passed through this worktree donate its
  badge: `gh pr checkout <n>` on someone else's merged PR and a switch back
  would badge the workspace with that PR for the whole reflog window. Both the
  query (`gh pr list --state open`, which includes drafts) and the client-side
  selector enforce it, so the two cannot drift.
- **Cost.** One repo-wide `gh pr list` matched client-side against every
  candidate (not one query per candidate), memoized per origin URL for 60s, and
  the local probes memoized per `(worktree, branch)` for the same 60s. Both
  caches matter: the `gh` memo alone still left a `git reflog` — plus a
  `git config` read per candidate — running on every 5s active-workspace tick.
  Owner resolution is also lazy, so it costs `git config` only for a candidate
  that already matched an open PR. On a repeat tick within the TTL the whole
  fallback spawns nothing. A branch switch is a new cache key and is answered
  at once; only a PR opened on an already-scanned branch waits out the TTL,
  which is the latency the memoized `gh` list imposed anyway.
- **A side-branch association is a badge and nothing more.** The workspace
  snapshot carries `pr_head_branch` (snapshot-local, serde-defaulted, never
  synced) alongside `pr_number`/`pr_state`/`pr_url`, and the frontend predicate
  `isPrOnCurrentBranch` (in `pr-status-icon.tsx`, shared because neither the
  sidebar nor the hover card may import the other) compares it to `git_branch`.
  Everything that *renders* the PR ignores the distinction; everything that
  *draws a conclusion* from it asks first:
  - **auto-settle** refuses the merged/closed shortcut for a side-branch PR.
    That branch merging says nothing about the checkout in front of the user,
    which may still be full of uncommitted work. Such a workspace still ages
    out through the ordinary inactivity rule — the guard removes the shortcut,
    not the workspace's eventual settlement.
  - the **"Wrapping up" tier** likewise refuses it: an open PR off a side
    branch is not this workspace winding down.
  - the **Review tab** stays strictly current-branch (see
    `docs/features/review-integration.md`) — it is a working surface, not a
    badge, and honouring a side-branch PR there would hide "Create PR" for the
    branch the user is actually on.

  **Missing information on either side reads as a match.** A `null` head branch
  means "association predates the field", not "side branch", so old persisted
  snapshots settle exactly as they used to. A `null` `git_branch` is the same
  unknown from the other end — `git_branch_info` reports no branch during a
  rebase, a bisect, or any detached HEAD, while the stored head branch survives
  — and reading that as a mismatch would silently un-associate a workspace from
  its own open PR for the length of the rebase, down to offering "Create PR" in
  the Review tab. Only two *known*, differing names count as a side-branch
  association. The hover/details card adds a **"PR branch"** row only in that
  case, where it answers the question the badge raises.
- **The anti-oscillation invariant.** Three base lifecycles (active / settled /
  snoozed), the local keep-active auto-settle override, the durable workspace
  pin visibility override, and five park-mutating effects (auto-settle, the
  auto-un-settle safety net, the snooze hand-raise, the wake sweep, the precise
  wake timer) share one list, so the guards are what keep a row from being
  fought over. Each effect is keyed to a status band no other effect touches:
  - auto-settle refuses **working / permission** status. Idle and completed
    review work may settle; the focused workspace may settle on PR completion
    without navigating away.
  - the settle safety net (auto-un-settle) fires **only at working/permission**.
  - the snooze hand-raise fires **only at working/permission**; the wake sweep
    fires only on an elapsed wake time. Auto-settle refuses both live states,
    so the effects cannot claim the same workspace at once.
  - auto-settle skips snoozed ids entirely (they are not in `activeCards`), so
    a snoozed workspace can't be settled out from under its own wake timer; the
    store enforces the same exclusivity from the other side by dropping a
    snooze when a settle lands (and a settled entry when a snooze lands).
  - a **keep-active pin** blocks auto-settle until the agent shows real
    activity. Only a **new** non-null status edge clears it
    (`noteActivity`'s `clearPin`) — selecting a workspace, laying down a
    first-seen baseline (including after restart), or repeatedly observing the
    same completed status stamps but leaves the pin standing, and a *timer*
    wake touches no pin at all (a clock expiring says nothing about what the
    user wants).
    `unsettle`/`unsnooze` with reason `"user"` sets the pin; `"activity"`
    clears it; `"timer"` leaves it alone.
  - a **durable workspace pin** blocks all manual, bulk, and automatic
    **parking** while it is present. If the workspace was already parked when
    pinned, its shelf entry is retained but presentation is overridden, so
    unpinning reveals that lifecycle instead of a destroyed one.
    **Un-parking is deliberately not blocked.** A pin governs where a card is
    shown, not whether its agent went live or its wake time elapsed, so two
    effects still clear a preserved entry underneath a pinned card: the settle
    safety net (a settled workspace whose agent goes working/permission
    resurfaces) and the wake sweep / boundary timer (a snooze ticket that comes
    due wakes). Both only ever *remove* a shelf entry, never add one, so
    neither can fight the pin — the card stays on top either way, and unpinning
    then reveals the up-to-date lifecycle rather than a stale snooze. The one
    concession to the override is cosmetic: the **"Woke" pill is suppressed
    while a card is pinned**, because that badge announces a return to a list
    the pinned card never left.

  - the **"Wrapping up" tier is not a fifth state.** It mutates nothing — it is
    a pure partition of the cards auto-settle already left alone, so it can
    neither park a row nor be fought over by anything that can. Its PR
    condition (`open`) is also disjoint from auto-settle's (`merged`/`closed`),
    so the two rules can never claim the same card.

  So no two of these effects can act on the same workspace at the same status,
  and a user-kept or user-deferred card stays put until its agent genuinely runs
  again.
- **Unread marker**: derived, not stored. `isWorkspaceUnread` reports unread
  when `last_active_at > last_visited_at` (or when the workspace has been
  active but never visited); a workspace the backend has never seen active is
  never unread, because absence of history is not news. The card renders it as
  an **ember dot + bold title** on the title line — deliberately not on the
  eyebrow, since "the agent finished and you haven't looked" is a different
  claim from "Done · review", and it must survive the hover swap. The focused
  workspace is never unread whatever the stamps say. A **"Mark unread"** context
  entry adds a session-only override (there is no read flag to flip), offered
  only on cards that don't already read as unread; a real visit — which moves
  `last_visited_at` — is what clears it, so the override can't outlive the truth
  it overrides.
- **Multi-select and bulk parking**: Cmd/Ctrl-click toggles a row into the
  selection; Shift-click selects the range **over rendered rows only**
  (`selectRange` walks the visible id list — active cards in rendered order,
  i.e. top tier then wrapping-up tier, then the shelves — so rows behind "Show
  more" or inside a collapsed shelf are never silently included and a bulk menu's "Settle (12)"
  can't lie about what is on screen; a stale anchor degrades to the clicked
  row). Cards, snoozed rows, and settled rows all participate. A plain click
  activates *and* collapses the selection; the shelf chrome (a "Snoozed" /
  "Settled" disclosure header, "Show N more") is **not** a click on the list and
  leaves the ticks alone — a user reaching for either is looking for more rows
  to add. Collapsing a shelf still drops its rows out of `renderedIds`, so a
  bulk count stays honest about what is on screen, and re-opening the shelf
  brings those ticks straight back. Right-clicking inside a selection of
  two or more is intercepted in the **capture phase** — otherwise the row's own
  workspace menu opens first and quietly narrows the gesture back to one row —
  and opens a bulk menu anchored at the pointer offering **Settle (n)** and
  **Snooze (n)** (with the shared presets) — each only when *every* selected
  workspace can take it. Both verbs ride the per-row guardrail (`isSnoozeable`,
  the same predicate as the card's `canSettle`): a selection containing a
  working or permission-blocked workspace offers neither (a **monitoring**
  workspace is deliberately parkable — a watch loop is exactly the kind of
  thing a user defers), and the menu shows a
  disabled line saying why instead of rendering empty — a bulk gesture is not a
  license to park in a batch what no single row offers, and a bulk action that
  silently skipped the busy half would make its own count a lie. Changing the
  repo filter clears the selection.
- **Forward navigation on park**: settling or snoozing the workspace you are
  *looking at* moves you to the next surviving active card (wrapping past the
  end) — `nextWorkspaceAfterPark`. Parking a **background** workspace navigates
  nowhere; so does a background auto-settle sweep. A manual settle navigates
  immediately rather than after the 200ms collapse.
- **Settled-tail pagination and forced visibility**: 10 settled rows render
  initially; a quiet mono "Show N more (X hidden)" button appends 25 per click.
  Paging resets when the repo filter changes. Both shelves are **collapsible**
  via a keyboard-reachable disclosure header (Settled always shows its count;
  Snoozed shows its count only while collapsed) — Settled starts open, Snoozed
  starts closed. Whatever shelf the **currently-open workspace** lands on, its
  row is force-rendered even when the shelf is collapsed or the row is past the
  paging window (appended rather than spliced, so the head keeps its honest
  recency order): its highlight is the user's only "you are here", and its
  Un-settle / Wake button is the only way back out.
- **Keyboard jumps**: `Alt+1`–`Alt+9` (rebindable `workspaceJump1..9`
  registry actions) activate the Nth visible active card — filter-scoped,
  parked (settled *and* snoozed) rows excluded. The target list is the
  rendered order **across both tiers** (`[...topTier, ...wrappingUp]`), and
  the overlay badges are numbered from the same list, so a digit always
  matches the badge the user is looking at even when a card sits below the
  "Wrapping up" divider. Because the card order is
  newest-first and static, `Alt+1` is the **newest** workspace and holds that
  slot while agents start and stop. Holding the modifier overlays index badges on the
  first nine cards; the badge hint reads the user's *actual* resolved
  `workspaceJump1` binding rather than hardcoding Alt (rebinding to Ctrl makes
  Ctrl reveal the badges; a chord using neither Alt nor Ctrl shows no badges at
  all), and hints clear on `blur`/`visibilitychange`. `sidebar-inbox-jump.ts`
  holds the visual-order targets for the central keyboard handler.
  `Ctrl+1..9` remain terminal-tab switching.
- **Provider marks**: each card's meta line shows the official logo of every
  agent-chat provider active in that workspace (Claude / Codex / OpenCode) via
  `ProviderLogo` + `getWorkspaceProviders` (pane-status). Terminal-only agent
  panes carry no provider metadata and contribute nothing.
- **Status derivation**: agent state comes from `getWorkspaceStatus`
  (pane-status) — covering terminal and Agent Chat panes alike. The **card's**
  idle elapsed label still comes from the non-persisted `sidebar-density-store`
  status observations; the **shelves'** labels do not (settled rows read
  `resolveSettledTimestamp`, snoozed rows `formatTimeUntil`).
- **Clocks**: one shared `useCoarseClock` (~30s, `Date.now()` at render time)
  drives every elapsed label plus the activity, auto-settle, and wake-sweep
  effects. The only finer timer is the snooze wake boundary described above.
  Snooze *presets* deliberately no longer ride this clock — they read
  `Date.now()` directly, at menu-open time.
- **Settings** (Settings → Appearance → Sidebar): **Show git stats**
  (`sidebar.show_git_stats`, default on) hides the `↑ahead` and `+/−` numbers
  on cards when off; the branch name always shows. The `sidebar.live_agents`
  grouping setting was removed with the tree; the working-indicator settings
  (`sidebar.working_indicator`, `sidebar.working_indicator_color`) remain and
  drive the card's working glyph.

### Footer nav (fixed chrome, both states)

`sidebar-footer-bar.tsx` is a slim app-destination row — never inside the
scrolling list. Expanded: **Automations** and **Workspaces** as equal-width
labeled ghost buttons (28px, 7px radius, transparent with a subtle hover
fill), then icon-only **Ports** (`SidebarPortsPopover`, keeps its count
badge) and the **Settings gear** (the app-menu dropdown — Settings, command
palette, shortcuts, documentation, report issue, version, sign out; its old
Automations/Workspaces items were removed since they're visible buttons now).
Collapsed: the same four, restacked vertically in the same order — Automations,
Workspaces, and the app menu get right-side tooltips; `SidebarPortsPopover`
hardcodes `side="top"` for both its tooltip and its popover in both states.

### Rail rendering (collapsed — 52px workspace strip)

Replaced the old project-avatar rail (aggregate dots + hover flyout,
`sidebar-rail-projects.tsx`, deleted):

- **Header** (`sidebar-action-row.tsx` collapsed variant): the neutral-ghost
  new-agent square (same treatment as the expanded header's pencil — no accent
  fill) + a search icon (opens the command palette), then a slim centered
  divider. Add repository lives only in the expanded filter row.
- **Workspace strip** (`sidebar-rail-workspaces.tsx`): one 28px button per
  **active or pinned workspace** — repo avatar with that workspace's own
  status dot (red pulse = needs you, amber = working, green = done-review,
  none = idle), and the shared right-side **hover card** (it replaced the
  title-only tooltip — a 28px avatar is the sidebar's least legible surface,
  so it benefits most). Clicking selects the
  workspace **without expanding**; the selected button gets the neutral
  border + lighter fill. The strip scrolls (scrollbar hidden); the repo filter
  does not apply here. **Both** parking lifecycles hide a button — settled and
  snoozed alike, matching the expanded inbox, since a snoozed workspace still
  holding a rail button is the deferral undone. The one exception is also the
  inbox's: the **currently-open** workspace keeps its button whichever shelf it
  is parked on, because the selection fill is the collapsed sidebar's only
  "you are here". A pinned workspace is the other exception: it stays visible
  even when its preserved base lifecycle is parked, carries a small pin badge,
  and sorts into a pinned block ahead of every normal rail button. Buttons run
  **newest-first within each block** via the same
  `compareNewestFirst` the expanded inbox sorts with (imported from
  `sidebar-inbox.tsx` — one implementation, shared so the two views can never
  disagree), so collapsing the sidebar never re-shuffles the order the user
  just memorized. The rail has no Wrapping-up tier or divider.
  It mirrors the cards' **background recede**: a button that is not the open
  workspace, is not unread (`isWorkspaceUnread` on the two backend stamps,
  imported from `sidebar-inbox.tsx` — the rail has no manual-unread override),
  and whose agent is working or idle drops to `opacity-70`, restored on hover.
  The status dot dims with the button on purpose. There is no "Woke" term in
  the predicate here, since the rail carries no woke marker.
- **Footer**: Automations, Workspaces, Ports (badge), Settings — same order
  as the expanded footer row.
- **Setup banner** (`sidebar-setup-banner.tsx`): hidden in the rail.

### Selection and update cost

Two properties keep the sidebar cheap at real profile scale (the audited
79-workspace profile). Both are invariants rather than incidental
optimizations — see `docs/plans/gui-responsiveness.md` for the program they
came from.

- **Selection is optimistic.** Every activation surface — inbox card, rail
  button, needs-you jump-link, ports popover, resource monitor row, command
  palette, workspaces overview, `Alt+1..9` — goes through the single
  `activateWorkspaceInteraction` helper (`src/lib/perf/instrumented-activate.ts`)
  rather than calling `activateWorkspace` directly. The helper writes a
  **pending** active id into `app-store` synchronously, in the click's own task,
  and only then invokes Rust, so the highlight moves before the round trip
  instead of waiting on the snapshot, the IPC hop, and the emit coalescer. The
  exported `selectActiveWorkspaceId` resolves that pending id **only while the
  workspace still exists in the snapshot**, falling back to
  `appState.active_workspace_id` otherwise, so a pending id for a
  deleted/archived workspace can never blank the selection. A rejected invoke
  rolls back, id-scoped (`clearPendingActivation(id)` no-ops when a newer click
  already moved the pending id, so a late failure cannot cancel a newer
  selection), and a 5 s timeout is the backstop for a reply that never comes.
  `prevWorkspace` / `nextWorkspace` are the one exception — they still run the
  Rust `cycle_workspace` command, which now performs the same side effects as
  `activate_workspace`.
- **A backend tick re-renders one card, not the list.** `SidebarInbox`
  subscribes to `appState.workspaces`, `appState.pane_statuses` and a boolean
  "have we loaded state" flag instead of the whole snapshot, and
  `SidebarInboxCard`, `SettledRow` and `SnoozeRow` are `memo()`d with
  `useCallback`'d handlers over a latched ref and interned `InboxRepo` objects.
  The backend's high-frequency metadata now arrives as ordered
  `app-state-delta` messages (`workspace_git`, `detected_ports`, `pane_status`)
  that replace only the touched workspace's object rather than rebuilding the
  snapshot, so one workspace's git sweep result re-renders exactly that card.
  This is asserted, not assumed: `sidebar-inbox-delta.test.tsx` drives a real
  delta through the store and fails if a sibling card re-renders.

## What Works Today

- Two-state toggle (expanded inbox ↔ icon rail) via title-bar button and `Ctrl+B`.
- Flat inbox cards with live agent state, blocker lines, git/PR/issue/remote/
  notification detail, and work-based titling while an agent is live.
- Durable Pin/Unpin with a pinned-first card block, static creation order,
  direct card Unpin, archive/restore preservation, and settled/snoozed
  visibility override without lifecycle data loss.
- Project dropdown filtering active + snoozed + settled lists (with active
  counts); pinned add-repo button; sticky filter row.
- A pinned, filter-scoped "Needs you" strip of jump-links in the sticky
  header — oldest-blocked-first, capped at 4 with an honest `+N more below`
  overflow and true total count.
- A "Wrapping up" tier that demotes (never hides) an open-PR card once its
  agent is idle and the user has read it, and returns it to the top tier the
  moment a follow-up puts the agent back to work.
- Settle/un-settle with ~200ms motion, persisted across restarts, prune-safe.
- Snooze with DST-safe presets resolved at menu-open time and labelled with the
  concrete local wake instant, a persisted wake time, an overflow-clamped wake
  timer, an early hand-raise wake on working/permission, and a "Woke" badge.
- Off-screen rows skipped for layout and paint (`content-visibility: auto`) on
  all three repeating shapes, with the rows still mounted.
- Backend-owned `last_active_at` / `last_visited_at` stamps that survive
  restarts and reinstalls, with a one-time boot backfill from git history.
- Unread markers derived from those stamps, plus a "Mark unread" override.
- Background recede on cards and rail buttons: quietly-working and idle-read
  rows sit at reduced opacity so the current, needs-you, done-review, unread,
  woke and multi-selected rows are the bright ones; hover/focus restores.
- Cmd/Ctrl-click and Shift-click multi-select with a bulk Settle/Snooze menu.
- Forward navigation when parking the workspace you are currently viewing.
- Search affordance opening the command palette; neutral-ghost new-agent button.
- Show git stats toggle (Settings → Appearance → Sidebar).
- Rail: one avatar button per active or pinned workspace, pinned block first
  then newest-first within each block, with pin badges, individual status dots,
  select-without-expand, and the shared footer destinations.
- Hover details on every workspace surface (card, settled row, snoozed row,
  rail avatar) — full title, complete git picture, PR/issue, ports, device,
  and path on disk.
- Per-workspace agent status covers both terminal and Agent Chat agents
  (chat sessions publish into the same `pane_statuses` snapshot).
- The done-review checkmark **survives an app restart**. `save_persisted_state`
  keeps `PaneStatus::Review` entries in `layout.json` and drops
  `Working`/`Permission`/`Monitoring` (dead processes after a quit) plus
  entries whose pane no longer exists — see `retain_persistable_pane_statuses` in
  `src-tauri/src/state/state_impl.rs`. Previously the whole map was cleared on
  every save, so finished workspaces came back looking already-reviewed. The
  badge still clears the normal way, on activating the workspace/tab.

## Current Constraints

- The collapsed/expanded choice is **not persisted across app restarts** (the app
  boots expanded), matching the pre-existing behavior.
- The rail is desktop-only; on the mobile breakpoint the sidebar still uses the
  off-canvas `Sheet`.
- Drag-and-drop workspace/project reordering was an affordance of the removed
  project tree and does not exist in the inbox. Card order is fixed:
  newest-first by stored snapshot index within each tier, with no user control
  over it and no way to opt a card out of (or into) the "Wrapping up" tier
  other than by working in it.
- Unmounting the project tree took two **project-level** surfaces offline, since
  both hung off its context menu. **Project avatar image/color customization**
  has since been re-homed onto the workspace context menu (see "Project section"
  under the inbox model above, and `docs/features/project-avatars.md`).
  **Archive Project** (`docs/features/workspace-archive.md`) is still offline and
  has no inbox equivalent — re-homing it is outstanding work.
- The **card's** idle elapsed label still comes from `settledAt` in the
  non-persisted `sidebar-density-store`, which is stamped only on a transition
  into `review` — so a workspace that went working → idle without ever reaching
  review, or any workspace after an app restart, shows no elapsed label on its
  card. The backend now persists `last_active_at`, which would fix this; the
  card just doesn't read it yet (the settled shelf does, via `workEndedAt`).
  The hover card's elapsed reading inherits exactly this limitation (it reads
  the same store, ticking on the shared coarse clock while the card is open).
- Shelf collapse state (Settled open, Snoozed closed) and the settled paging
  window are **component state** — not persisted, and reset on every mount.
- Multi-selection is session-only and cleared by a plain click, an activation,
  or a filter change; there is no select-all and no keyboard entry into it.
- `manualUnread` ("Mark unread") is session-only for the same reason unread is
  derived rather than stored — nothing persists it across a restart.
- Backend stamps are **device-local**. Ordinary creation paths write
  `last_active_at = now`, which is honest — the workspace really is new here.
  The **adoption shells** (`create_synced_workspace_shell` /
  `create_synced_root_shell`) are the exception and leave it `None`, since a
  workspace pulled from another device has a history that simply happened
  elsewhere; the backfill then dates the arrived checkout from its own git
  log. That correction lands **a launch late**: the shell carries `host_id`
  until its pull succeeds, and the backfill skips host-backed workspaces, so
  it is the *next* boot pass that dates it. Until then the stamp is `null`,
  which reads as "unknown" and never auto-sweeps.
- **Attach-only and still-remote workspaces never get a stamp at all.** Their
  paths name directories on another machine, so the backfill declines to probe
  them; they stay `null` and therefore never auto-sweep.
- The collapsed rail intentionally shows only each workspace's agent-status
  dot — the old project-avatar rail's notification-count badges were not
  carried over (notification detail lives on the expanded cards).
- The rail carries no snooze, unread, or "Woke" affordance — parking state
  reaches it only as an absence (the button is gone), never as a marker.
- The superseded tree components (`sidebar-project-group.tsx`,
  `sidebar-live-section.tsx`, `sidebar-live-grouping.ts`, and the
  `SidebarWorkspaceRow` component) are still
  in the repo but **unmounted**. Only `sidebar-workspace-row.tsx` is load-bearing:
  the inbox menu (`workspace-inbox-menu.tsx`) imports its `WorkspaceContextMenuItems`
  / `DeleteWorktreeDialog` and its `SettleMenuAction` / `SnoozeMenuAction` types
  plus the lifecycle block those drive (Un-settle / Wake now / Settle /
  "Snooze until…" / Mark unread, ordered so the entries that bring work *back*
  come first) — the `SidebarWorkspaceRow` *component* in that
  module is itself unmounted. The other files (`sidebar-project-group.tsx`,
  `sidebar-live-section.tsx`, `sidebar-live-grouping.ts`, and
  `workspace-reorder.ts` — the old drag-reorder pure transform, which the inbox
  has no equivalent for) are pure dead code; each still has a test suite except
  `sidebar-live-section.tsx`, which has none. Removing them is a pending cleanup.
  `sidebar-needs-you-strip.tsx` is **no longer dead** — it is re-mounted in the
  inbox's sticky header (see "Needs you strip" above).

## Load-Bearing Assumptions

Things that look incidental and are not. Breaking any of these breaks the inbox
quietly rather than loudly.

- **`last_visited_at` is stamped only by `record_workspace_switch`** — which
  stamps both the incoming and the outgoing workspace, and which every mover of
  `active_workspace_id` (`activate_workspace`, plus `activate_terminal_session`
  and `activate_pane` on *cross-workspace* switches) calls. Not by
  same-workspace pane focus, not by rendering. Adding a writer that fires on
  every pane focus would silently disable the unread marker, since unread is
  `last_active_at > last_visited_at`; dropping the outgoing stamp brings back
  "you get marked unread for work you just watched"; and moving
  `active_workspace_id` anywhere *without* the helper reopens the hole where a
  jump-to-session switch left the visit ledger untouched.
- **Idle never stamps `last_active_at`.** `set_pane_status(Idle)` removes the
  pane entry and writes nothing. If idle ever stamped, the idle sweep would
  measure "time since the agent stopped stopping" and nothing would ever settle.
- **`backfill_workspace_activity` runs once, on a background thread, chained
  onto `backfill_workspace_protection`.** It shells out to `git` per workspace,
  so it must never move onto the app-startup path; it shares that one boot
  thread deliberately (both passes walk every checkout and then take the state
  lock, so back-to-back costs one thread instead of two contending). It is
  self-limiting — it only probes workspaces still missing a stamp — and it
  re-checks `is_none()` under the lock before writing, because a live agent may
  have stamped a real value while the git work ran unlocked.
- **`derive_last_activity_ms` returns `None`, never `now`.** `null` means
  "unknown" and the frontend refuses to sweep on it. Making it fall back to
  `now` would exempt every stale workspace from the idle sweep for a full window
  after each update — the bug the backend field was added to kill.
- **The backend stamp must keep winning in `effectiveActivityAt`.** Flipping the
  precedence resurrects the synthetic first-seen baselines left in the client
  `activity` map by pre-`last_active_at` installs.
- **`SnoozeMenuAction.offered` is the "snooze not offered" signal.** It replaced
  "an empty preset array means not offered", which stopped being expressible the
  moment presets started being resolved inside the menu — there is no array left
  to be empty. The card sets it from `canSnooze`; a snoozed row passes `false`
  (its entry is "Wake now", which needs no presets). A caller that hardcodes
  `true` would surface a Snooze trigger in states where snoozing is guardrailed
  off.
- **`canSnooze` is derived from `canSettle`, not restated.** A second copy of
  the working/permission condition is exactly how a later edit opens a hole that
  lets live or blocked work be buried. It is now the *only* gate on the card's
  Snooze trigger, since the preset list no longer doubles as one.
- **The repeating row shapes are CSS-contained, not virtualized.** The card
  wrapper, the snoozed row, and the settled row each carry
  `content-visibility: auto` with a `contain-intrinsic-size` hint (88px for the
  card wrapper — its measured resting height including the 6px gap; 30px exactly
  for both shelf rows, which is their literal `h-[30px]` with no margin). Rows
  stay **mounted**, so in-page find, focus and tab order, the `Alt+1..9` jump
  targets, the forced-visible "you are here" row and every per-row effect keep
  working — a virtualizer would break all of those for a list that is normally
  well under a few hundred rows. The intrinsic sizes are load-bearing: a hint
  that disagrees with the real height makes the scrollbar jump as rows are
  realised on the way down, so re-measure before changing card padding, the
  meta line, or the row height.
- **The hand-raise needs no staleness check, and `SnoozeEntry.at` is not one.**
  Nothing reads `at` — it is only the record of when the deferral was made.
  What makes a "wake on activity that predates the snooze" unreachable is
  `isSnoozeable` (a working or blocked workspace cannot be snoozed in the first
  place) plus `pane_statuses` being runtime-only — the backend clears the map
  before persisting, so no stale status survives a restart. Any
  working/permission a *snoozed* workspace shows is therefore a transition that
  happened after the snooze. Weakening either of those makes an instant re-wake
  reachable, and the hand-raise would then need a real timestamp comparison.
- **`resolveSettledTimestamp` is the single key for both the shelf's sort and
  its label.** Splitting them lets a row sit in one place while claiming a
  different time.
- **The settle timer is flushed on unmount.** `pendingSettleRef` exists because
  collapsing the sidebar mid-gesture cancels the 200ms timeout; without the
  flush the settle is silently dropped.
- **`sidebar.inbox.settled` is a historical key name.** It now holds snooze
  state too. Renaming it strands every existing install's shelf.
- **`isWrappingUp` must keep all three conditions.** Dropping the unread check
  buries the one card the user came to the sidebar for; dropping the status
  check breaks "send a follow-up and it comes back up"; accepting `draft`
  demotes work the author has explicitly marked as still in progress; accepting
  `merged`/`closed` puts it in a tug-of-war with auto-settle over the same card.
- **Never gate a historical PR on exact local/remote head-SHA equality.** A
  review commit can legitimately advance the remote PR head beyond a workspace
  that remains open locally. The branch/repository identity owns the
  association; `headRefOid` is metadata only. Also keep lookup errors distinct
  from successful empty results, or an offline/rate-limited poll will erase a
  correct badge.
- **The jump targets and range selection read `orderedActiveCards`, not
  `activeCards`.** Both describe positions the user can *see*, so they must be
  `[...topTier, ...wrappingUp]`. Reverting either to the unpartitioned list
  makes `Alt+3` land on a different card than the badge shows and lets a
  shift-click range cover rows the user never dragged across.
- **The "Wrapping up" divider is not a `ShelfHeader`.** These are active,
  actionable cards; giving the section a disclosure control would offer to hide
  work that is nearly-but-not-done, which is precisely what the tier exists to
  avoid doing.
- **Every activation surface must route through `activateWorkspaceInteraction`.**
  A site that calls `activateWorkspace` directly still works, but loses the
  synchronous pending id — so that one surface silently reverts to
  "selection waits for the backend", and it contributes no interaction trace.
  The helper is the only place the pending id, its rollback, and its 5 s
  timeout are managed; adding a second writer of `pendingActiveWorkspaceId`
  reopens the race the id-scoped clear exists to close.
- **`useCoarseClock` must keep its timestamp in state.** It used to return a
  fresh `Date.now()` on every render, which changed a prop on every parent
  render and silently defeated *every* memo boundary beneath it — the cards
  looked memoized and re-rendered anyway. It now ticks state on one shared
  ~30 s interval; reverting it to a per-render read would undo the row
  memoization without touching any `memo()` call.
- **Bulk actions resolve against `renderedIds`.** Selection, ranges, and the
  menu's counts all describe rows currently on screen; widening them to the full
  list would let a bulk action hit workspaces the user never saw.

## Important Touch Points

- `src/components/layout/app-sidebar.tsx` — `collapsible="icon"`, rail overflow override
- `src/components/layout/sidebar-workspace-list.tsx` — expanded → inbox, collapsed → rail
- `src/components/layout/sidebar-inbox.tsx` — filter dropdown, card list, the two active tiers (`isWrappingUp` + `WrappingUpDivider`), both shelves, the settle/snooze/wake/auto-settle effects and their invariants, multi-select, park navigation, `compareNewestFirst` (shared with the rail)
- `src/components/layout/sidebar-needs-you-strip.tsx` — the pinned needs-you strip of jump-links in the sticky header (oldest-blocked-first, capped at 4)
- `src/components/layout/sidebar-inbox-card.tsx` — the workspace card + Settle/Snooze action pair + unread/Woke markers + context menu wiring
- `src/components/layout/sidebar-snooze.ts` — pure, clock-free wake presets + `formatWakeLabel` + `formatTimeUntil`
- `src/components/layout/workspace-inbox-menu.tsx` — the shared right-click menu for all three row shapes
- `src/components/layout/workspace-hover-card.tsx` — the shared hover-details card for cards, settled/snoozed rows, and rail avatars
- `src/components/layout/project-appearance-menu.tsx` — the `Project "<name>"` submenu (image + accent color)
- `src/stores/project-appearance-store.ts` — shared project avatar appearance (one writer, many readers)
- `src/components/layout/sidebar-inbox-jump.ts` — visual-order jump targets for Alt+1..9
- `src/lib/keybind-registry.ts` — `workspaceJump1..9` actions (default `Alt+1..9`)
- `src/lib/use-coarse-clock.ts` — the single ~30s clock behind every elapsed label and sweep
- `src/components/settings/settings-view.tsx` — the Appearance → Sidebar subsection
- `src/stores/sidebar-inbox-store.ts` — persisted `{settled, snoozed, keepActive, activity}` blob, pin rules, `resolveSettledTimestamp`, session repo filter
- `src/tauri/types.ts` — `WorkspaceSnapshot.last_active_at` / `last_visited_at`
- `src-tauri/src/state/state_impl.rs` — `stamp_workspace_activity` (non-idle pane transitions), `record_workspace_switch` (the only `last_visited_at` writer, shared by `activate_workspace` / `activate_terminal_session` / `activate_pane`), `backfill_workspace_activity` / `derive_last_activity_ms`
- `src-tauri/src/lib.rs` — the boot thread that runs `backfill_workspace_protection`, which chains the activity backfill
- `src/components/layout/sidebar-action-row.tsx` — expanded search/new-agent header + collapsed rail header
- `src/components/layout/sidebar-rail-workspaces.tsx` — collapsed per-workspace strip
- `src/components/layout/sidebar-footer-bar.tsx` — footer nav (Automations/Workspaces/Ports/app menu)
- `src/components/layout/sidebar-workspace-row.tsx` — shared `WorkspaceContextMenuItems` + `DeleteWorktreeDialog` + `SettleMenuAction` / `SnoozeMenuAction` (the row component itself is unmounted)
- `src/components/layout/use-project-appearance.ts` — shared avatar appearance loader
- `src/components/ui/sidebar.tsx` — width defaults (288px expanded, 52px rail)
- `src/components/ui/working-indicator.tsx` — configurable working indicator
- `src/stores/sidebar-density-store.ts` — non-persisted status-transition timestamps
- `src/lib/pane-status.ts` — `getWorkspaceStatus` / `getProjectStatus` helpers
- `src/stores/app-store.ts` — project grouping + duplicate-label disambiguation

## Notes

- Keep this file about current truth, not future plans.
- The inbox was implemented from the `Sidebar Inbox.dc.html` design handoff
  (flat inbox of workspace cards); colors map to the existing status/accent
  tokens — no hardcoded palette values.
