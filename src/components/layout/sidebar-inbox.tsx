import { startTransition, useEffect, useMemo, useRef, useState } from "react";
import {
  AlarmClock,
  AlertCircle,
  ChevronDown,
  ChevronRight,
  Folder,
  FolderOpen,
  FolderPlus,
  GitMerge,
  Loader2,
  Undo2,
} from "lucide-react";
import { cn } from "@/lib/utils";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import {
  useAppStore,
  useHomeDir,
  useProjectGroupedWorkspaces,
} from "@/stores/app-store";
import { useHosts } from "@/stores/hosts-store";
import { useUIStore } from "@/stores/ui-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import {
  useSidebarInboxStore,
  resolveSettledTimestamp,
} from "@/stores/sidebar-inbox-store";
import {
  useSettingsStore,
  selectSidebarShowGitStats,
  selectSidebarAutoSettleDays,
} from "@/stores/settings-store";
import { formatElapsed } from "@/stores/sidebar-density-store";
import { getWorkspaceStatus } from "@/lib/pane-status";
import { useCoarseClock } from "@/lib/use-coarse-clock";
import { useProjectActions } from "@/hooks/use-project-actions";
import { useProjectAppearance } from "./use-project-appearance";
import { SidebarInboxCard, type InboxRepo } from "./sidebar-inbox-card";
import { WorkspaceInboxMenu } from "./workspace-inbox-menu";
import {
  computeSnoozePresets,
  formatTimeUntil,
  type SnoozePreset,
} from "./sidebar-snooze";
import { WorkspaceHoverCard } from "./workspace-hover-card";
import { activateWorkspace } from "@/tauri/commands";
import {
  normalizePrState,
  type PrStatusState,
} from "@/components/github/pr-status-icon";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";
import { parseKeyCombo } from "@/lib/keybind-utils";
import {
  setJumpTargets,
  DEFAULT_JUMP_MODIFIER,
} from "./sidebar-inbox-jump";
import type { ActivePaneStatus, WorkspaceSnapshot } from "@/tauri/types";

/** How many leading cards get a jump badge — the digit shortcuts only reach 1-9. */
const MAX_JUMP_HINTS = 9;

/** How long the settle collapse runs before the card actually moves below
 *  the divider. Matches the card wrapper's `duration-200`. */
const SETTLE_ANIM_MS = 200;
/** How long the rise-in ease on a just-settled / just-un-settled row is kept
 *  before the marker clears. */
const ROW_IN_MS = 400;

/** Settled-tail paging: show a short head on first paint, then reveal a larger
 *  page at a time so the settled section can never dominate the sidebar. */
const SETTLED_INITIAL_COUNT = 10;
const SETTLED_PAGE_COUNT = 25;

/** How long a merged/closed-PR workspace must ALSO have been idle before the
 *  sweep parks it.
 *
 *  Without this the merge signal is permanent: the moment status drops to null
 *  the card vanishes under the divider, so returning to a merged workspace for
 *  follow-up work (a review comment, a revert, a cherry-pick) un-settles it
 *  only until the agent stops — then it snaps straight back out of sight,
 *  taking the conversation the user was reading with it. Measuring against the
 *  same activity stamp the idle rule uses keeps a warm workspace visible and
 *  parks it only once it has genuinely gone quiet. */
export const MERGED_PR_SETTLE_IDLE_MS = 3_600_000;

/** The largest delay `setTimeout` can hold. The delay is stored as a signed
 *  32-bit int, so anything past this silently overflows to a negative value and
 *  the timer fires *immediately* — a "Next week" snooze would wake on the spot.
 *  Clamping instead re-arms at the ceiling and the effect schedules the
 *  remainder on its next pass. */
export const MAX_TIMER_DELAY_MS = 2_147_483_647;

/** Clamp a wake delay into the range `setTimeout` can actually represent.
 *  Negative / non-finite input means the boundary has already passed, which is
 *  a fire-now, not an error. */
export function clampTimerDelay(ms: number): number {
  if (!Number.isFinite(ms) || ms <= 0) return 0;
  return Math.min(ms, MAX_TIMER_DELAY_MS);
}

/** Newest workspace first.
 *
 *  The backend has no `created_at`, but it *appends* new workspaces to the
 *  snapshot array, so a later stored index is a newer workspace. Reversing that
 *  index puts the thing the user just started at the top of the inbox, where
 *  they are already looking.
 *
 *  Crucially this order is **static**: it is derived only from creation order,
 *  never from status, activity, or notification counts. A row therefore holds
 *  its position from the moment it opens until it settles or snoozes, so the
 *  list moves only at lifecycle transitions the user caused. That is what makes
 *  `Alt+1..9` a muscle-memory shortcut rather than a lottery, and what stops
 *  rows from swapping under the pointer every time an agent starts or stops.
 *  Status is not lost — it is carried by each card's own state cluster. */
export function compareNewestFirst(
  a: { storedIndex: number },
  b: { storedIndex: number },
): number {
  return b.storedIndex - a.storedIndex;
}

/** Whether a card belongs to the "Wrapping up" tier — still active, still
 *  fully actionable, just no longer worth the top of the list.
 *
 *  Opening a PR is a *wind-down* signal, not a completion one. The work is
 *  handed off, but review comments, conflicts and follow-ups all still arrive
 *  through that workspace, so it must stay on screen and keep every gesture it
 *  had. All this does is stop it occupying the real estate the user's live work
 *  needs. Merged/closed PRs are a different claim entirely and are left to the
 *  auto-settle sweep and its idle guard.
 *
 *  Each condition earns its place:
 *  • **`"open"`, never `"draft"`.** A draft PR is a PR the author is still
 *    writing against — pushing it down would demote work that is still in
 *    progress by the author's own declaration.
 *  • **Status null.** Anything the agent is currently doing outranks the PR,
 *    and this is also what gives "reopen it and send a follow-up" its
 *    behaviour for free: the follow-up makes the agent work, status goes
 *    non-null, and the card climbs back to the top tier by itself. No separate
 *    "unsettled again" flag to set, get wrong, or leave stuck.
 *  • **Not unread.** This is the one that matters. Unread means the agent
 *    produced something the user has not looked at yet, and demoting that
 *    buries the exact thing they opened the sidebar to find. The PR says "this
 *    is nearly done"; unread says "you don't know that yet". The second claim
 *    wins until they visit, at which point the card drifts down on its own.
 *
 *  Note what is *not* in here: elapsed time, notification counts, how recently
 *  a pane blinked. Membership flips only on durable lifecycle events — a PR
 *  opening, an agent run the user caused, the user reading the result — so the
 *  tier is as static as the order inside it (see `compareNewestFirst`) and
 *  can't churn a card across the divider while the user is aiming at it. */
export function isWrappingUp(
  prState: PrStatusState | null,
  status: ActivePaneStatus | null,
  unread: boolean,
): boolean {
  return prState === "open" && status === null && !unread;
}

/** Where to move the user after parking (settling / snoozing) workspaces.
 *
 *  Parking the workspace you are *looking at* leaves the main pane showing a
 *  card that is no longer in the list, so we step forward to the next surviving
 *  active card (wrapping past the end). Parking a background workspace returns
 *  null — yanking the user out of what they are reading because a sweep touched
 *  some other row would be the single most hostile thing this list could do.
 *  Null also means "nothing left to move to"; the caller then does nothing. */
export function nextWorkspaceAfterPark(
  parkedIds: readonly string[],
  activeWorkspaceId: string,
  activeIds: readonly string[],
): string | null {
  if (!parkedIds.includes(activeWorkspaceId)) return null;
  const index = activeIds.indexOf(activeWorkspaceId);
  if (index === -1) return null;
  const parked = new Set(parkedIds);
  for (let step = 1; step <= activeIds.length; step += 1) {
    const candidate = activeIds[(index + step) % activeIds.length];
    if (!parked.has(candidate)) return candidate;
  }
  return null;
}

/** Whether a workspace has agent output the user has not looked at yet.
 *
 *  Purely derived from the two backend stamps — there is no "read" flag to
 *  flip, which is also why the manual "Mark unread" override has to be carried
 *  separately by the caller (see `manuallyMarked`). A workspace the backend has
 *  never seen active is never unread: absence of history is not news. */
export function isWorkspaceUnread(
  lastActiveAt: number | null | undefined,
  lastVisitedAt: number | null | undefined,
  manuallyMarked = false,
): boolean {
  if (manuallyMarked) return true;
  if (lastActiveAt == null) return false;
  return lastVisitedAt == null || lastActiveAt > lastVisitedAt;
}

/** The instant a workspace was last genuinely worked in, as the idle sweep
 *  should measure it.
 *
 *  The backend stamp wins outright whenever it exists. It is persisted, it
 *  survives updates and reinstalls, and it is derived from real agent
 *  transitions rather than from whenever this window happened to first render.
 *
 *  The client map is only a fallback for workspaces the backend could not date
 *  at all (no commits, no readable checkout). It must never override the
 *  backend, and the reason is a migration one: installs predating
 *  `last_active_at` wrote a synthetic `Date.now()` baseline into the client map
 *  for every workspace they had. Those stamps are indistinguishable from real
 *  activity, so honouring them would make a machine full of month-old work look
 *  brand new for a full idle window after every single update — which is the
 *  exact bug this field exists to kill. Preferring the backend retires that
 *  polluted state without needing a migration pass over it. */
export function effectiveActivityAt(
  backendAt: number | null | undefined,
  clientAt: number | undefined,
): number | undefined {
  return backendAt ?? clientAt;
}

/** Whether a workspace in this state may be deferred.
 *
 *  Same guardrail as Settle, for the same reason: a working agent or one
 *  blocked on a permission prompt can never be put out of sight. Snooze is
 *  worse than settle here — settle at least leaves the row on a shelf the user
 *  is looking at, while a snooze hides it behind a wake time. */
export function isSnoozeable(status: ActivePaneStatus | null): boolean {
  return status !== "working" && status !== "permission";
}

/** The ids between two rows in the list the user can actually see.
 *
 *  Range selection is resolved against the *rendered* ids only. Selecting rows
 *  hidden behind "Show more" or inside a collapsed shelf would make the bulk
 *  menu's "Settle (12)" a lie about what is on screen, and the user would be
 *  acting on workspaces they never saw. An anchor that is no longer rendered
 *  degrades to selecting just the clicked row. */
export function selectRange(
  renderedIds: readonly string[],
  anchorId: string,
  targetId: string,
): string[] {
  const target = renderedIds.indexOf(targetId);
  if (target === -1) return [];
  const anchor = renderedIds.indexOf(anchorId);
  if (anchor === -1) return [targetId];
  const lo = Math.min(anchor, target);
  const hi = Math.max(anchor, target);
  return renderedIds.slice(lo, hi + 1);
}

/** How a click on a row translates into selection intent. Plain clicks still
 *  activate the workspace; the modifiers are the only way into bulk mode. */
type SelectMode = "single" | "toggle" | "range";

function selectModeFor(e: {
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
}): SelectMode {
  if (e.metaKey || e.ctrlKey) return "toggle";
  if (e.shiftKey) return "range";
  return "single";
}

/** Activate a workspace from a sidebar row. Clearing the chat draft first
 *  matches the card path — a stale draft would follow the user across. */
function activateFromSidebar(workspaceId: string): void {
  useChatDraftStore.getState().setActiveDraft(null);
  startTransition(() => {
    activateWorkspace(workspaceId).catch(console.error);
  });
}

/** A project's mini square avatar, resolving its customised appearance. Kept
 *  as a per-item component so the appearance hook always runs unconditionally
 *  (each avatar owns its own hook rather than looping hooks in the parent). */
function ProjectMiniAvatar({ name, path }: { name: string; path: string }) {
  const appearance = useProjectAppearance(path);
  return (
    <ProjectAvatar
      name={name}
      color={appearance.customColor}
      imageUrl={appearance.imageUrl}
      cacheBust={appearance.imageVersion}
      size="sm"
      shape="square"
      className="font-bold"
    />
  );
}

interface ProjectFilterItemProps {
  name: string;
  path: string;
  /** Number of active (unsettled) workspaces in this project. */
  count: number;
  active: boolean;
  onSelect: () => void;
}

/** One project row in the filter dropdown: mini avatar, name, and its active
 *  workspace count. */
function ProjectFilterItem({
  name,
  path,
  count,
  active,
  onSelect,
}: ProjectFilterItemProps) {
  return (
    <DropdownMenuItem
      onClick={onSelect}
      aria-label={name}
      className={cn(
        "h-8 gap-2 rounded-[7px] px-2 text-xs font-semibold",
        active && "bg-foreground/[0.08] text-foreground",
      )}
    >
      <ProjectMiniAvatar name={name} path={path} />
      <span className="min-w-0 flex-1 truncate">{name}</span>
      <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground">
        {count}
      </span>
    </DropdownMenuItem>
  );
}

/** Header for the Snoozed / Settled shelves. Collapsing is a plain disclosure
 *  button rather than a hover affordance so the section state is discoverable
 *  and keyboard-reachable. */
function ShelfHeader({
  label,
  count,
  showCount,
  collapsed,
  onToggle,
}: {
  label: string;
  count: number;
  /** The Snoozed shelf hides its count while expanded — the rows are right
   *  there to be counted. Settled always shows it. */
  showCount: boolean;
  collapsed: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onToggle}
      aria-expanded={!collapsed}
      aria-label={`${label} (${count})`}
      className="flex w-full items-center gap-2 px-1 pb-1.5 pt-3 outline-none"
    >
      <ChevronRight
        aria-hidden="true"
        className={cn(
          "size-2.5 shrink-0 text-muted-foreground/70 transition-transform duration-150",
          !collapsed && "rotate-90",
        )}
      />
      <span className="font-mono text-[9.5px] uppercase tracking-[0.13em] text-muted-foreground/70">
        {label}
      </span>
      {showCount && (
        <span className="font-mono text-[9.5px] tabular-nums text-muted-foreground/70">
          ({count})
        </span>
      )}
      <span className="h-px flex-1 bg-border/60" />
    </button>
  );
}

/** The label above the "Wrapping up" tier.
 *
 *  Deliberately *not* `ShelfHeader`. That component is a disclosure button, and
 *  everything about it — the chevron, `aria-expanded`, the tab stop — promises
 *  that the rows below can be folded away. These rows can't and mustn't: they
 *  are ordinary active cards that happen to be winding down, and a control that
 *  offers to hide live-but-nearly-done work is the bug this tier exists to
 *  avoid. Sharing the typography and the hairline rule keeps it in the same
 *  visual family as the Snoozed / Settled headers (the user reads it as "a
 *  section starts here") while the missing chevron says the rest. */
function WrappingUpDivider() {
  return (
    <div
      data-wrapping-up-divider
      className="flex w-full items-center gap-2 px-1 pb-1.5 pt-3"
    >
      <span className="font-mono text-[9.5px] uppercase tracking-[0.13em] text-muted-foreground/70">
        Wrapping up
      </span>
      <span aria-hidden="true" className="h-px flex-1 bg-border/60" />
    </div>
  );
}

interface SettledRowProps {
  workspace: WorkspaceSnapshot;
  repo: InboxRepo;
  isActive: boolean;
  selected: boolean;
  /** Live agent status for the hover card — a settled workspace can still be
   *  running (e.g. a "review" agent stays settled), so the row must not hard-
   *  code idle. */
  status: ActivePaneStatus | null;
  /** Elapsed-since-work-ended label ("2h"), or null when unknown. */
  time: string | null;
  justSettled: boolean;
  onUnsettle: (workspaceId: string) => void;
  onSelect: (workspaceId: string, mode: SelectMode) => void;
  onMarkUnread: (workspaceId: string) => void;
}

function SettledRow({
  workspace,
  repo,
  isActive,
  selected,
  status,
  time,
  justSettled,
  onUnsettle,
  onSelect,
  onMarkUnread,
}: SettledRowProps) {
  const appearance = useProjectAppearance(repo.path);
  const merged = normalizePrState(workspace.pr_state) === "merged";

  const handleClick = (e: React.MouseEvent) => {
    const mode = selectModeFor(e);
    onSelect(workspace.workspace_id, mode);
    // Modifier clicks are selection gestures only — activating as well would
    // scroll the main pane away mid-multi-select.
    if (mode !== "single") return;
    activateFromSidebar(workspace.workspace_id);
  };

  return (
    // Same right-click menu as the active cards (with Un-settle on top), so
    // a settled row keeps every workspace action — archive, rename, delete,
    // move-to-host — without having to un-settle it first.
    <WorkspaceInboxMenu
      workspace={workspace}
      repo={repo}
      settleAction={{
        kind: "unsettle",
        onAction: () => onUnsettle(workspace.workspace_id),
      }}
      unreadAction={{
        onMarkUnread: () => onMarkUnread(workspace.workspace_id),
      }}
    >
    {/* Settled rows show only a title, so the hover card carries even more
        weight here than on an active card. The bare div is intentional: it
        takes the ContextMenuTrigger's `asChild` so the hover trigger below
        never composes onto the same node, and the settled list is a plain
        block container, so the extra block wrapper is layout-neutral. */}
    <div>
    <WorkspaceHoverCard workspace={workspace} repo={repo} status={status}>
    <div
      role="button"
      tabIndex={0}
      data-settled-row={workspace.workspace_id}
      data-selected={selected ? "true" : undefined}
      onClick={handleClick}
      onKeyDown={(e) => {
        if (e.key !== "Enter" && e.key !== " ") return;
        // Space would otherwise scroll the sidebar as it activates the row.
        if (e.key === " ") e.preventDefault();
        // Collapse any multi-select first, exactly like a plain click —
        // keyboard navigation must not leave an invisible selection behind
        // for the next bulk action to act on.
        onSelect(workspace.workspace_id, "single");
        activateFromSidebar(workspace.workspace_id);
      }}
      className={cn(
        "group/settled flex h-[30px] cursor-pointer items-center gap-2 rounded-lg px-2",
        // Same off-screen containment as the cards (see `SidebarInboxCard`):
        // the Settled shelf is the list that actually grows without bound, and
        // paging only limits what is *rendered*, not what the forced-visible
        // and expanded cases can reach. The hint is exact here — the row's
        // height is the literal `h-[30px]` above and it carries no margin — so
        // scroll position can't drift as rows are realised.
        "[content-visibility:auto] [contain-intrinsic-size:auto_30px]",
        "outline-none transition-colors duration-150 hover:bg-foreground/[0.045] focus-visible:bg-foreground/[0.045]",
        isActive && "bg-foreground/[0.06]",
        selected && "bg-foreground/[0.09] ring-1 ring-inset ring-border",
        justSettled && "rise-in",
      )}
    >
      <ProjectAvatar
        name={repo.name}
        color={appearance.customColor}
        imageUrl={appearance.imageUrl}
        cacheBust={appearance.imageVersion}
        size="sm"
        shape="square"
        className="font-bold opacity-80"
      />
      {merged && (
        <GitMerge
          aria-label="PR merged"
          className="h-3 w-3 shrink-0 text-accent-violet"
        />
      )}
      <span className="min-w-0 flex-1 truncate text-xs font-medium text-muted-foreground">
        {workspace.title}
      </span>
      {time && (
        <span className="shrink-0 text-[10.5px] text-muted-foreground/70 group-hover/settled:hidden group-focus-within/settled:hidden">
          {time}
        </span>
      )}
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          onUnsettle(workspace.workspace_id);
        }}
        aria-label={`Un-settle "${workspace.title}"`}
        className={cn(
          "hidden h-[19px] shrink-0 items-center gap-1 rounded-md border border-border bg-muted px-[7px]",
          "text-[10px] font-semibold text-muted-foreground transition-colors duration-150",
          "hover:border-muted-foreground/50 hover:text-foreground",
          "group-hover/settled:inline-flex group-focus-within/settled:inline-flex",
        )}
      >
        <Undo2 className="h-2.5 w-2.5" />
        Un-settle
      </button>
    </div>
    </WorkspaceHoverCard>
    </div>
    </WorkspaceInboxMenu>
  );
}

interface SnoozeRowProps {
  workspace: WorkspaceSnapshot;
  repo: InboxRepo;
  isActive: boolean;
  selected: boolean;
  /** Live agent status for the hover card — same contract as `SettledRow`.
   *  A snoozed "review" agent stays snoozed (only working/permission wake it
   *  early), so the row must not hard-code idle. */
  status: ActivePaneStatus | null;
  /** Time until the workspace comes back ("3h", "2d") — a snoozed row's whole
   *  story is its return ticket, so it shows time-until, not time-since. */
  timeUntil: string;
  onWake: (workspaceId: string) => void;
  onSelect: (workspaceId: string, mode: SelectMode) => void;
  onMarkUnread: (workspaceId: string) => void;
}

/** One deferred workspace on the Snoozed shelf. Deliberately the same one-line
 *  shape as a settled row — both are "parked", and giving them different
 *  silhouettes would suggest a difference in kind rather than in duration. */
function SnoozeRow({
  workspace,
  repo,
  isActive,
  selected,
  status,
  timeUntil,
  onWake,
  onSelect,
  onMarkUnread,
}: SnoozeRowProps) {
  const appearance = useProjectAppearance(repo.path);

  const handleClick = (e: React.MouseEvent) => {
    const mode = selectModeFor(e);
    onSelect(workspace.workspace_id, mode);
    if (mode !== "single") return;
    activateFromSidebar(workspace.workspace_id);
  };

  return (
    <WorkspaceInboxMenu
      workspace={workspace}
      repo={repo}
      snoozeAction={{
        // A snoozed row can only come back — re-deferring it is the "Snooze
        // until…" gesture of an *active* card, so no presets are needed here.
        kind: "wake",
        offered: false,
        onSnooze: () => {},
        onWake: () => onWake(workspace.workspace_id),
      }}
      unreadAction={{
        onMarkUnread: () => onMarkUnread(workspace.workspace_id),
      }}
    >
    {/* Same hover-details coverage as a settled row — a snoozed row is just as
        lossy (one line, no meta), and the same bare-div nesting keeps the
        ContextMenuTrigger and the hover trigger off one shared node. */}
    <div>
    <WorkspaceHoverCard workspace={workspace} repo={repo} status={status}>
      <div
        role="button"
        tabIndex={0}
        data-snoozed-row={workspace.workspace_id}
        data-selected={selected ? "true" : undefined}
        onClick={handleClick}
        onKeyDown={(e) => {
          if (e.key !== "Enter" && e.key !== " ") return;
          if (e.key === " ") e.preventDefault();
          // Same selection collapse as a plain click (see `SettledRow`).
          onSelect(workspace.workspace_id, "single");
          activateFromSidebar(workspace.workspace_id);
        }}
        className={cn(
          "group/snoozed flex h-[30px] cursor-pointer items-center gap-2 rounded-lg px-2",
          // Identical silhouette to a settled row, so identical containment
          // hint — 30px exactly, no margin.
          "[content-visibility:auto] [contain-intrinsic-size:auto_30px]",
          "outline-none transition-colors duration-150 hover:bg-foreground/[0.045] focus-visible:bg-foreground/[0.045]",
          isActive && "bg-foreground/[0.06]",
          selected && "bg-foreground/[0.09] ring-1 ring-inset ring-border",
        )}
      >
        <ProjectAvatar
          name={repo.name}
          color={appearance.customColor}
          imageUrl={appearance.imageUrl}
          cacheBust={appearance.imageVersion}
          size="sm"
          shape="square"
          className="font-bold opacity-60"
        />
        <AlarmClock
          aria-hidden="true"
          className="h-3 w-3 shrink-0 text-muted-foreground/60"
        />
        <span className="min-w-0 flex-1 truncate text-xs font-medium text-muted-foreground">
          {workspace.title}
        </span>
        <span
          aria-label={`Wakes in ${timeUntil}`}
          className="shrink-0 text-[10.5px] tabular-nums text-muted-foreground/70 group-hover/snoozed:hidden group-focus-within/snoozed:hidden"
        >
          {timeUntil}
        </span>
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onWake(workspace.workspace_id);
          }}
          aria-label={`Wake "${workspace.title}" now`}
          className={cn(
            "hidden h-[19px] shrink-0 items-center gap-1 rounded-md border border-border bg-muted px-[7px]",
            "text-[10px] font-semibold text-muted-foreground transition-colors duration-150",
            "hover:border-muted-foreground/50 hover:text-foreground",
            "group-hover/snoozed:inline-flex group-focus-within/snoozed:inline-flex",
          )}
        >
          <AlarmClock className="h-2.5 w-2.5" />
          Wake now
        </button>
      </div>
    </WorkspaceHoverCard>
    </div>
    </WorkspaceInboxMenu>
  );
}

/** The flat workspace inbox that replaces the nested project tree in the
 *  expanded sidebar: a project filter dropdown, one multi-line card per active
 *  workspace (newest first), a "Snoozed" shelf of deferred rows, and a
 *  "Settled" section of one-line rows the user has swept aside. Parking a
 *  workspace is visual only — nothing is archived or deleted. */
export function SidebarInbox() {
  const appState = useAppStore((s) => s.appState);
  const allWorkspaces = useMemo(
    () => appState?.workspaces ?? [],
    [appState?.workspaces],
  );
  const paneStatuses = appState?.pane_statuses;
  const activeWorkspaceId = appState?.active_workspace_id ?? "";
  const homeDir = useHomeDir();
  const hosts = useHosts();
  const projectGroups = useProjectGroupedWorkspaces(allWorkspaces, homeDir, hosts);
  const pendingWorkspaces = useUIStore((s) => s.pendingWorkspaces);
  const setShowNewProjectScreen = useUIStore((s) => s.setShowNewProjectScreen);
  const { openProject } = useProjectActions();

  const showGitStats = useSettingsStore(selectSidebarShowGitStats);
  const autoSettleDays = useSettingsStore(selectSidebarAutoSettleDays);

  const load = useSidebarInboxStore((s) => s.load);
  const loaded = useSidebarInboxStore((s) => s.loaded);
  const settled = useSidebarInboxStore((s) => s.settled);
  const snoozed = useSidebarInboxStore((s) => s.snoozed);
  const keepActive = useSidebarInboxStore((s) => s.keepActive);
  const activity = useSidebarInboxStore((s) => s.activity);
  const filter = useSidebarInboxStore((s) => s.filter);
  const setFilter = useSidebarInboxStore((s) => s.setFilter);
  const prune = useSidebarInboxStore((s) => s.prune);

  // One coarse (~30s) clock for every elapsed label in the list, and the tick
  // the activity + auto-settle + wake effects re-run on.
  const now = useCoarseClock(true);

  useEffect(() => {
    void load();
  }, [load]);

  // Drop settled entries whose workspace vanished (archived / deleted).
  useEffect(() => {
    if (!loaded || !appState) return;
    prune(new Set(allWorkspaces.map((w) => w.workspace_id)));
  }, [loaded, appState, allWorkspaces, prune]);

  // Settle motion: collapse the card (~200ms), then flip the persisted flag
  // so it re-renders as a one-line row under the divider (which eases in).
  const [leavingId, setLeavingId] = useState<string | null>(null);
  const [justSettledId, setJustSettledId] = useState<string | null>(null);
  const [justUnsettledId, setJustUnsettledId] = useState<string | null>(null);

  // Shelf disclosure. Settled stays open — it is the history the user sweeps
  // into and reads back. Snoozed starts closed: its rows are, by definition,
  // work the user said they did not want to see right now, so re-showing them
  // every launch would undo the gesture.
  const [settledCollapsed, setSettledCollapsed] = useState(false);
  const [snoozeCollapsed, setSnoozeCollapsed] = useState(true);

  // Workspaces woken by the timer or by their own agent, so the card can badge
  // the return. Cleared on visit — once the user has looked, it isn't news.
  const [wokeIds, setWokeIds] = useState<ReadonlySet<string>>(new Set());
  // "Mark unread" override. Unread is *derived* from two backend stamps, so
  // there is no flag to flip; the only honest local counterpart is a small set
  // of ids we force-report as unread until the user next visits them. The
  // tradeoff: it is session-only and a real visit (which moves
  // `last_visited_at`) is what clears it, so the override can never outlive
  // the truth it is overriding.
  const [manualUnread, setManualUnread] = useState<ReadonlySet<string>>(new Set());

  // Multi-select for bulk parking. Ids only — the rows are re-derived every
  // render, so holding on to snapshots would go stale.
  const [selectedIds, setSelectedIds] = useState<ReadonlySet<string>>(new Set());
  const [selectionAnchor, setSelectionAnchor] = useState<string | null>(null);
  // Where to anchor the bulk right-click menu (viewport coords of the click).
  const [bulkMenuAt, setBulkMenuAt] = useState<{ x: number; y: number } | null>(
    null,
  );

  // Visiting a workspace answers both markers at once.
  useEffect(() => {
    if (!activeWorkspaceId) return;
    setWokeIds((prev) => {
      if (!prev.has(activeWorkspaceId)) return prev;
      const next = new Set(prev);
      next.delete(activeWorkspaceId);
      return next;
    });
    setManualUnread((prev) => {
      if (!prev.has(activeWorkspaceId)) return prev;
      const next = new Set(prev);
      next.delete(activeWorkspaceId);
      return next;
    });
  }, [activeWorkspaceId]);

  // Settled-tail paging window. Reset to the short head whenever the repo
  // filter changes so a newly-scoped list starts collapsed again. Settle /
  // unsettle deliberately don't reset it — the slice just shows fewer rows.
  const [settledVisibleCount, setSettledVisibleCount] = useState(
    SETTLED_INITIAL_COUNT,
  );
  useEffect(() => {
    setSettledVisibleCount(SETTLED_INITIAL_COUNT);
    // A selection made under one filter describes rows that are no longer on
    // screen; carrying it across would let a bulk action hit workspaces the
    // user can't see.
    setSelectedIds(new Set());
    setSelectionAnchor(null);
  }, [filter]);
  const timeoutsRef = useRef<number[]>([]);
  // The workspace whose settle is still waiting behind the collapse animation.
  // Unmounting (e.g. collapsing the sidebar) cancels the timer, so the pending
  // settle is flushed here instead of being silently dropped.
  const pendingSettleRef = useRef<{ id: string; workEndedAt?: number } | null>(
    null,
  );
  useEffect(
    () => () => {
      timeoutsRef.current.forEach((t) => window.clearTimeout(t));
      const pending = pendingSettleRef.current;
      if (pending !== null) {
        pendingSettleRef.current = null;
        useSidebarInboxStore.getState().settle(pending.id, pending.workEndedAt);
      }
    },
    [],
  );

  /** Keep the just-settled / just-un-settled rise-in marker for one beat. */
  const markRowIn = (
    setter: (updater: (cur: string | null) => string | null) => void,
    workspaceId: string,
  ) => {
    setter(() => workspaceId);
    timeoutsRef.current.push(
      window.setTimeout(
        () => setter((cur) => (cur === workspaceId ? null : cur)),
        ROW_IN_MS,
      ),
    );
  };

  // ── Derived lists ──

  // workspace_id → repo (project) identity, from the same grouping pipeline
  // the rest of the app uses (dedup'd names, Home labeling, host suffixes).
  const repoByWorkspace = useMemo(() => {
    const map = new Map<string, InboxRepo>();
    for (const group of projectGroups) {
      for (const ws of group.workspaces) {
        map.set(ws.workspace_id, {
          name: group.projectName,
          path: group.projectPath,
        });
      }
    }
    return map;
  }, [projectGroups]);

  const settledIds = useMemo(
    () => new Set(settled.map((e) => e.id)),
    [settled],
  );
  const snoozedIds = useMemo(
    () => new Set(snoozed.map((e) => e.id)),
    [snoozed],
  );

  const workspaceById = useMemo(() => {
    const map = new Map<string, WorkspaceSnapshot>();
    for (const ws of allWorkspaces) map.set(ws.workspace_id, ws);
    return map;
  }, [allWorkspaces]);

  const matchesFilter = (ws: WorkspaceSnapshot) =>
    !filter || repoByWorkspace.get(ws.workspace_id)?.path === filter;

  const statusOf = (ws: WorkspaceSnapshot) =>
    paneStatuses ? getWorkspaceStatus(ws.surfaces, paneStatuses) : null;

  // Active cards: newest workspace on top, and the order never moves for any
  // reason short of a lifecycle transition (see `compareNewestFirst`). The
  // repo filter scopes all three lists.
  const activeCards = allWorkspaces
    .map((ws, storedIndex) => ({ ws, storedIndex }))
    .filter(
      ({ ws }) =>
        !settledIds.has(ws.workspace_id) &&
        !snoozedIds.has(ws.workspace_id) &&
        matchesFilter(ws),
    )
    .sort(compareNewestFirst)
    .map(({ ws }) => ws);

  /** The workspace you are looking at is never unread, whatever the backend
   *  stamps say — the visit is happening right now. Shared by the card and by
   *  the wrapping-up partition below so the two can never disagree about
   *  whether there is unseen news on a row. */
  const isUnread = (ws: WorkspaceSnapshot) =>
    ws.workspace_id !== activeWorkspaceId &&
    isWorkspaceUnread(
      ws.last_active_at,
      ws.last_visited_at,
      manualUnread.has(ws.workspace_id),
    );

  // Two tiers, one list. Both are ordinary active cards with every gesture
  // intact; the split only decides who gets the top of the sidebar. Each tier
  // keeps `activeCards`' newest-first order because we partition the sorted
  // list rather than re-sorting, so within a tier nothing moves either.
  const topTier: WorkspaceSnapshot[] = [];
  const wrappingUpTier: WorkspaceSnapshot[] = [];
  for (const ws of activeCards) {
    const tier = isWrappingUp(
      normalizePrState(ws.pr_state),
      statusOf(ws),
      isUnread(ws),
    )
      ? wrappingUpTier
      : topTier;
    tier.push(ws);
  }
  // Everything downstream that means "the active cards, top to bottom" reads
  // this and not `activeCards`: the Alt+1..9 jump targets, range selection, and
  // the post-park forward navigation all describe positions on screen, so a
  // list in a different order than the DOM would make Alt+3 land on the wrong
  // card and a shift-click select rows the user never dragged across.
  const orderedActiveCards = [...topTier, ...wrappingUpTier];

  // Snoozed rows, soonest wake first — the shelf reads as a queue of returns.
  const snoozedRows = snoozed
    .map((entry) => ({ entry, workspace: workspaceById.get(entry.id) }))
    .filter(
      (r): r is { entry: (typeof snoozed)[number]; workspace: WorkspaceSnapshot } =>
        r.workspace !== undefined && matchesFilter(r.workspace),
    )
    .sort((a, b) => a.entry.until - b.entry.until);

  // Settled rows, newest work first. Sort key and label both come from
  // `resolveSettledTimestamp`, so a row's position and its elapsed label can
  // never tell two different stories.
  const settledRows = settled
    .map((entry) => ({ entry, workspace: workspaceById.get(entry.id) }))
    .filter(
      (r): r is { entry: (typeof settled)[number]; workspace: WorkspaceSnapshot } =>
        r.workspace !== undefined && matchesFilter(r.workspace),
    )
    .sort(
      (a, b) =>
        resolveSettledTimestamp(b.entry) - resolveSettledTimestamp(a.entry),
    );

  // The open workspace is never allowed to hide. Whatever shelf it landed on,
  // and whether that shelf is collapsed or paged, its row stays rendered — its
  // selection highlight is the user's only "you are here", and its un-settle /
  // wake button is the only way back out.
  const forcedVisibleId = activeWorkspaceId;

  const visibleSnoozed = snoozeCollapsed
    ? snoozedRows.filter((r) => r.workspace.workspace_id === forcedVisibleId)
    : snoozedRows;

  const settledHead = settledRows.slice(0, settledVisibleCount);
  const forcedSettledIndex = settledRows.findIndex(
    (r) => r.workspace.workspace_id === forcedVisibleId,
  );
  const pagedSettled =
    forcedSettledIndex >= settledVisibleCount
      ? // Appended rather than spliced in place: the head keeps its honest
        // recency order and the forced row reads as the exception it is.
        [...settledHead, settledRows[forcedSettledIndex]]
      : settledHead;
  const visibleSettled = settledCollapsed
    ? settledRows.filter((r) => r.workspace.workspace_id === forcedVisibleId)
    : pagedSettled;
  const settledHidden = settledRows.length - visibleSettled.length;

  const filteredPending = filter
    ? pendingWorkspaces.filter((pw) => pw.projectPath === filter)
    : pendingWorkspaces;

  const activeCardIds = orderedActiveCards.map((ws) => ws.workspace_id);
  // Every row the user can currently see, in visual order — the universe for
  // range selection and for the bulk menu's counts.
  const renderedIds = [
    ...activeCardIds,
    ...visibleSnoozed.map((r) => r.workspace.workspace_id),
    ...visibleSettled.map((r) => r.workspace.workspace_id),
  ];

  // Wake times for the bulk menu, resolved when that menu opens rather than on
  // the coarse clock. The list used to share one array recomputed every minute,
  // which made a preset up to a tick stale at the moment it was clicked and
  // pushed a new array identity through every card on each tick. Nothing in the
  // list needs presets now — only the two menus that can actually snooze do,
  // and each resolves its own.
  const [bulkSnoozePresets, setBulkSnoozePresets] = useState<SnoozePreset[]>(
    [],
  );

  // ── Park / un-park handlers ──

  const navigateAfterPark = (parkedIds: readonly string[]) => {
    const next = nextWorkspaceAfterPark(
      parkedIds,
      activeWorkspaceId,
      activeCardIds,
    );
    if (next) activateFromSidebar(next);
  };

  const handleSettle = (workspaceId: string) => {
    const workEndedAt =
      workspaceById.get(workspaceId)?.last_active_at ?? undefined;
    setLeavingId(workspaceId);
    pendingSettleRef.current = { id: workspaceId, workEndedAt };
    // Navigate now, not after the collapse: the user asked to move on, and
    // watching their own card animate away first just adds 200ms of nothing.
    navigateAfterPark([workspaceId]);
    timeoutsRef.current.push(
      window.setTimeout(() => {
        pendingSettleRef.current = null;
        useSidebarInboxStore.getState().settle(workspaceId, workEndedAt);
        setLeavingId(null);
        markRowIn(setJustSettledId, workspaceId);
      }, SETTLE_ANIM_MS),
    );
  };

  const handleUnsettle = (workspaceId: string) => {
    // The settled-row button is an explicit "keep this active" — pin it so the
    // idle/PR auto-settle rules leave it alone until its agent runs again.
    useSidebarInboxStore.getState().unsettle(workspaceId, "user");
    markRowIn(setJustUnsettledId, workspaceId);
  };

  const handleSnooze = (workspaceId: string, until: number) => {
    navigateAfterPark([workspaceId]);
    useSidebarInboxStore.getState().snooze(workspaceId, until);
  };

  /** Wake a snoozed workspace back into the active list. `reason` follows the
   *  store's pin rules — only an explicit "Wake now" is a user decision. */
  const wake = (
    workspaceId: string,
    reason: "user" | "activity" | "timer",
  ) => {
    useSidebarInboxStore.getState().unsnooze(workspaceId, reason);
    markRowIn(setJustUnsettledId, workspaceId);
    // A manual wake needs no badge — the user is the one who did it.
    if (reason === "user") return;
    setWokeIds((prev) => {
      if (prev.has(workspaceId)) return prev;
      const next = new Set(prev);
      next.add(workspaceId);
      return next;
    });
  };

  // Un-settle a workspace because its live agent resurfaced it (not a user
  // gesture): clear any keep-active pin via reason "activity", then run the
  // same rise-in animation the manual path uses.
  const resurface = (workspaceId: string) => {
    useSidebarInboxStore.getState().unsettle(workspaceId, "activity");
    markRowIn(setJustUnsettledId, workspaceId);
  };

  // ── Selection ──

  const handleSelect = (workspaceId: string, mode: SelectMode) => {
    if (mode === "single") {
      setSelectedIds(new Set());
      setSelectionAnchor(workspaceId);
      return;
    }
    if (mode === "toggle") {
      setSelectedIds((prev) => {
        const next = new Set(prev);
        if (next.has(workspaceId)) next.delete(workspaceId);
        else next.add(workspaceId);
        return next;
      });
      setSelectionAnchor(workspaceId);
      return;
    }
    setSelectedIds(
      new Set(selectRange(renderedIds, selectionAnchor ?? workspaceId, workspaceId)),
    );
  };

  const handleMarkUnread = (workspaceId: string) => {
    setManualUnread((prev) => {
      const next = new Set(prev);
      next.add(workspaceId);
      return next;
    });
  };

  // Selection in visual order, restricted to what is on screen — the bulk
  // menu's counts describe exactly the rows the user is looking at.
  const selection = renderedIds.filter((id) => selectedIds.has(id));
  // Snooze is only offered when EVERY selected workspace can take it: a bulk
  // action that silently skipped the busy half would be worse than one that
  // isn't offered, because the count would claim otherwise.
  const canBulkSnooze =
    selection.length > 0 &&
    selection.every((id) => {
      const ws = workspaceById.get(id);
      return ws !== undefined && isSnoozeable(statusOf(ws));
    });
  // Settle rides the same guardrail as Snooze (see `isSnoozeable` — the
  // per-row `canSettle` is the same predicate): a working or
  // permission-blocked workspace can never be parked, and a bulk gesture is
  // not a license to do in a batch what no single row offers.
  const canBulkSettle = canBulkSnooze;

  const closeBulkMenu = () => setBulkMenuAt(null);

  const handleBulkSettle = () => {
    // The menu hides Settle for unsettleable selections; this guard is the
    // seatbelt for any other route into the handler.
    if (!canBulkSettle) return;
    const store = useSidebarInboxStore.getState();
    navigateAfterPark(selection);
    for (const id of selection) {
      store.settle(id, workspaceById.get(id)?.last_active_at ?? undefined);
    }
    setSelectedIds(new Set());
    closeBulkMenu();
  };

  const handleBulkSnooze = (until: number) => {
    const store = useSidebarInboxStore.getState();
    navigateAfterPark(selection);
    for (const id of selection) store.snooze(id, until);
    setSelectedIds(new Set());
    closeBulkMenu();
  };

  // Right-clicking inside a multi-selection means "act on all of these", so we
  // intercept in the capture phase — the row's own workspace menu would
  // otherwise open first and quietly narrow the gesture back to one row.
  const handleListContextMenuCapture = (e: React.MouseEvent) => {
    if (selection.length < 2) return;
    const target = e.target as HTMLElement | null;
    const row = target?.closest?.(
      "[data-inbox-card],[data-snoozed-row],[data-settled-row]",
    );
    const id =
      row?.getAttribute("data-inbox-card") ??
      row?.getAttribute("data-snoozed-row") ??
      row?.getAttribute("data-settled-row");
    if (!id || !selectedIds.has(id)) return;
    e.preventDefault();
    e.stopPropagation();
    setBulkMenuAt({ x: e.clientX, y: e.clientY });
  };

  // ── Lifecycle effects ──

  // Settle safety net: a settled workspace whose agent goes live ("working")
  // or blocked ("permission") resurfaces into the active list automatically,
  // so live or blocked work can never stay buried under the divider. Finished
  // ("review") and idle settled rows stay put — only fresh activity resurfaces
  // them. Unsettle removes the entry from `settled`, so re-runs converge; we
  // iterate a snapshot and skip ids that aren't currently settled.
  useEffect(() => {
    if (!loaded || !paneStatuses) return;
    for (const entry of settled) {
      const ws = allWorkspaces.find((w) => w.workspace_id === entry.id);
      if (!ws) continue;
      const status = getWorkspaceStatus(ws.surfaces, paneStatuses);
      if (status === "working" || status === "permission") {
        resurface(entry.id);
      }
    }
  }, [loaded, paneStatuses, settled, allWorkspaces]);

  // Snooze safety net (the hand-raise): a snoozed workspace whose agent starts
  // working or blocks on a permission prompt wakes immediately, wake time or
  // not. A snooze defers *waiting*; it can never be allowed to hide an agent
  // that is stuck asking the user a question, or the user would sit staring at
  // an empty inbox while work blocks behind a shelf they collapsed yesterday.
  //
  // No "is this newer than the snooze?" check is needed. `isSnoozeable`
  // refuses to snooze a working/blocked workspace in the first place, and
  // pane statuses are runtime-only (the backend clears them before persisting,
  // so no stale status survives a restart) — so reaching working/permission
  // here is always a transition that happened after the snooze was set.
  useEffect(() => {
    if (!loaded || !paneStatuses) return;
    for (const entry of snoozed) {
      const ws = allWorkspaces.find((w) => w.workspace_id === entry.id);
      if (!ws) continue;
      const status = getWorkspaceStatus(ws.surfaces, paneStatuses);
      if (status === "working" || status === "permission") {
        wake(entry.id, "activity");
      }
    }
  }, [loaded, paneStatuses, snoozed, allWorkspaces]);

  // Wake sweep: anything whose return ticket has come due goes back to the
  // active list. Driven off the coarse clock (which is `Date.now()` at render
  // time, so the comparison is exact even though the tick is not).
  useEffect(() => {
    if (!loaded) return;
    for (const entry of snoozed) {
      if (now >= entry.until) wake(entry.id, "timer");
    }
  }, [loaded, snoozed, now]);

  // …and a precise timer at the soonest boundary, because the coarse clock
  // alone would make a wake up to a full tick late — a "In 1 hour" snooze
  // returning at 1h00m29s reads as a broken promise. Re-armed via the nonce
  // after each fire so a queue of wakes is walked one boundary at a time.
  const [wakeNonce, setWakeNonce] = useState(0);
  useEffect(() => {
    if (!loaded || snoozed.length === 0) return;
    let soonest = Infinity;
    for (const entry of snoozed) soonest = Math.min(soonest, entry.until);
    if (!Number.isFinite(soonest)) return;
    const timer = window.setTimeout(
      () => setWakeNonce((n) => n + 1),
      clampTimerDelay(soonest - Date.now()),
    );
    return () => window.clearTimeout(timer);
  }, [loaded, snoozed, wakeNonce]);

  // Activity observation: keep a client-side "last active" stamp per workspace
  // so the inactivity auto-settle rule has something to measure against. We
  // stamp when the agent is doing something (non-null status), when it's the
  // focused workspace, or when we've never recorded it before.
  //
  // The first-seen baseline prefers the backend's `last_active_at`: stamping
  // `Date.now()` there would throw away real idleness and restart every
  // workspace's clock on every app update, so a machine full of month-old work
  // would look brand new. Only when the backend has no stamp at all do we fall
  // back to now — that keeps the anti-mass-settle property (a workspace with no
  // usable history must never auto-settle the instant it is first seen).
  //
  // Only a non-null status counts as genuine agent activity, so only that path
  // passes `clearPin` — a keep-active pin must survive merely selecting the
  // workspace (or its first-seen baseline), otherwise the auto-settle sweep
  // below would immediately re-settle a card the user just kept active.
  useEffect(() => {
    if (!loaded || !paneStatuses) return;
    const noteActivity = useSidebarInboxStore.getState().noteActivity;
    const at = Date.now();
    for (const ws of allWorkspaces) {
      const status = getWorkspaceStatus(ws.surfaces, paneStatuses);
      const isActiveWs = ws.workspace_id === activeWorkspaceId;
      const unseen = activity[ws.workspace_id] === undefined;
      if (status === null && !isActiveWs && !unseen) continue;
      // A live agent, or a workspace the user is looking at, is active *now*.
      // Anything else here is a first sighting, which inherits real history.
      const stamp =
        status !== null || isActiveWs ? at : ws.last_active_at ?? at;
      noteActivity(ws.workspace_id, stamp, { clearPin: status !== null });
    }
  }, [loaded, paneStatuses, allWorkspaces, activeWorkspaceId, activity, now]);

  // Auto-settle: sweep an active card under the divider on its own once it is
  // safely idle — either its PR merged/closed AND it has since gone quiet, or
  // it has gone untouched past the user's idle window. No leaving animation
  // (this is a background sweep, not a gesture) and deliberately no forward
  // navigation: a sweep must never move the user.
  //
  // Anti-fight invariants — the inbox has four states (active / settled /
  // snoozed / pinned-active) and these guards make oscillation impossible:
  //   • auto-settle ONLY fires at status null (never live / blocked / review);
  //   • the auto-un-settle safety net ONLY fires at working/permission;
  //   • the snooze hand-raise ONLY fires at working/permission, and the wake
  //     sweep only at an elapsed wake time — neither can fire at status null,
  //     which is the only status auto-settle acts on;
  //   • auto-settle skips snoozed ids entirely (they aren't in `activeCards`),
  //     so a snoozed workspace can't be settled out from under its own wake
  //     timer; the store keeps the two shelves mutually exclusive from the
  //     other side by dropping a snooze when a settle lands;
  //   • a keep-active pin blocks auto-settle until the agent shows real
  //     activity (a non-null status un-pins via noteActivity's clearPin);
  //     selecting the workspace is not activity and leaves the pin standing,
  //     and a timer wake deliberately touches no pin at all;
  //   • the "Wrapping up" tier is not a fifth state and mutates nothing — it is
  //     a pure partition of the cards this sweep already left alone, so it can
  //     neither park a row nor be fought over by anything that can. An open PR
  //     is also outside this sweep's PR rule (which wants merged/closed), so
  //     the two never claim the same card.
  // So no two of these effects can act on the same workspace at the same
  // status, and a user-kept or user-deferred card stays put until its agent
  // genuinely runs again.
  useEffect(() => {
    if (!loaded || !paneStatuses) return;
    const store = useSidebarInboxStore.getState();
    const settledSet = new Set(settled.map((e) => e.id));
    const snoozedSet = new Set(snoozed.map((e) => e.id));
    for (const ws of allWorkspaces) {
      const id = ws.workspace_id;
      if (settledSet.has(id) || snoozedSet.has(id)) continue;
      if (keepActive[id]) continue;
      // Never sweep the workspace the user is currently looking at. This used
      // to fall out of the client stamp being refreshed on every tick for the
      // focused row; now that the backend stamp is authoritative it has to be
      // said out loud, or the open workspace could settle underneath its own
      // main pane while the user reads it.
      if (id === activeWorkspaceId) continue;
      const status = getWorkspaceStatus(ws.surfaces, paneStatuses);
      if (status !== null) continue;
      const prState = normalizePrState(ws.pr_state);
      const prDone = prState === "merged" || prState === "closed";
      const stamp = effectiveActivityAt(ws.last_active_at, activity[id]);
      const idleFor = stamp === undefined ? 0 : now - stamp;
      // A merged PR is a strong signal but not an instant one: the workspace
      // has to have gone quiet too, so follow-up work stays on screen while
      // it's warm.
      const prSwept =
        prDone && stamp !== undefined && idleFor >= MERGED_PR_SETTLE_IDLE_MS;
      const idleSwept =
        autoSettleDays !== null &&
        stamp !== undefined &&
        idleFor > autoSettleDays * 86_400_000;
      if (prSwept || idleSwept) {
        store.settle(id, ws.last_active_at ?? undefined);
        markRowIn(setJustSettledId, id);
      }
    }
  }, [
    loaded,
    paneStatuses,
    allWorkspaces,
    settled,
    snoozed,
    keepActive,
    activity,
    activeWorkspaceId,
    autoSettleDays,
    now,
  ]);

  // ── Jump-to-card shortcuts ──
  // Publish the visible active-card ids (view order, filter-scoped) so the
  // window-level keyboard handler can resolve "jump to workspace N" without
  // coupling to React. Settled and snoozed rows are never jump targets. Keyed
  // on a joined string so the effect only re-runs when the visible set changes.
  const activeCardIdsKey = activeCardIds.join(" ");
  useEffect(() => {
    setJumpTargets(activeCardIdsKey ? activeCardIdsKey.split(" ") : []);
  }, [activeCardIdsKey]);
  useEffect(() => () => setJumpTargets([]), []);

  // Which physical modifier reveals the jump badges. Respect the user's actual
  // resolved binding for slot 1 (so a rebind to Ctrl/Alt tracks the right key);
  // fall back to the default modifier. A rebind to a non-Alt/Ctrl chord (e.g.
  // Shift-only) simply shows no held-modifier hints.
  const { keybindMap } = useResolvedKeybinds();
  const jumpModifierKey = useMemo(() => {
    const keys =
      keybindMap.get("workspaceJump1")?.activeKeys ??
      `${DEFAULT_JUMP_MODIFIER}+1`;
    const parsed = parseKeyCombo(keys);
    if (parsed.ctrl) return "Control";
    if (parsed.alt) return "Alt";
    return null;
  }, [keybindMap]);

  // Show the badges only while the modifier is physically held. Clear on keyup,
  // blur, and visibilitychange so the hints can never get stuck open.
  const [jumpHintsVisible, setJumpHintsVisible] = useState(false);
  useEffect(() => {
    if (!jumpModifierKey) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === jumpModifierKey) setJumpHintsVisible(true);
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === jumpModifierKey) setJumpHintsVisible(false);
    };
    const clear = () => setJumpHintsVisible(false);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", clear);
    document.addEventListener("visibilitychange", clear);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("blur", clear);
      document.removeEventListener("visibilitychange", clear);
      setJumpHintsVisible(false);
    };
  }, [jumpModifierKey]);

  // Whether the project filter dropdown is open — drives the trigger chevron
  // rotation.
  const [filterMenuOpen, setFilterMenuOpen] = useState(false);

  // Active (unsettled) workspace count per project, plus the grand total for
  // the "All projects" row. Derived from the same grouping + settled data the
  // lists use, so the badges always match what the filter would show. Parking
  // is the only thing that decrements a count — a wrapping-up card is still an
  // active workspace, and dropping it from the total would report work as gone
  // the moment its PR opened.
  const projectCounts = useMemo(() => {
    const map = new Map<string, number>();
    let total = 0;
    for (const group of projectGroups) {
      let count = 0;
      for (const ws of group.workspaces) {
        if (!settledIds.has(ws.workspace_id) && !snoozedIds.has(ws.workspace_id)) {
          count += 1;
        }
      }
      map.set(group.projectPath, count);
      total += count;
    }
    return { map, total };
  }, [projectGroups, settledIds, snoozedIds]);

  const filterName = filter
    ? projectGroups.find((g) => g.projectPath === filter)?.projectName ?? null
    : null;

  // The filtered project can disappear underneath us (its last workspace was
  // archived / deleted). Without this the trigger would render a blank label
  // and the list an unexplained empty state, so fall back to "All projects".
  useEffect(() => {
    if (!appState || filter === null) return;
    if (projectGroups.some((g) => g.projectPath === filter)) return;
    setFilter(null);
  }, [appState, filter, projectGroups, setFilter]);

  /** One active card. Both tiers render the identical component — a card that
   *  is wrapping up loses no affordance, only altitude. `visualIndex` is its
   *  position in `orderedActiveCards`, i.e. counted straight through the
   *  divider, so the badge a user sees while holding the jump modifier is the
   *  digit that actually activates that card. */
  const renderCard = (ws: WorkspaceSnapshot, visualIndex: number) => {
    const repo = repoByWorkspace.get(ws.workspace_id);
    if (!repo) return null;
    const id = ws.workspace_id;
    return (
      <SidebarInboxCard
        key={id}
        workspace={ws}
        repo={repo}
        isActive={id === activeWorkspaceId}
        status={statusOf(ws)}
        showGitStats={showGitStats}
        now={now}
        leaving={leavingId === id}
        justUnsettled={justUnsettledId === id}
        jumpHint={
          jumpHintsVisible && visualIndex < MAX_JUMP_HINTS
            ? visualIndex + 1
            : null
        }
        onSettle={handleSettle}
        onSnooze={handleSnooze}
        unread={isUnread(ws)}
        woke={wokeIds.has(id)}
        selected={selectedIds.has(id)}
        onSelect={handleSelect}
        onMarkUnread={handleMarkUnread}
      />
    );
  };

  return (
    <div className="flex flex-col">
      {/* Project filter — sticky so the filter stays reachable while the card
          list scrolls beneath it. */}
      <div className="sticky top-0 z-10 flex items-center gap-1.5 bg-sidebar px-2.5 pb-2.5 pt-0.5 min-w-0">
        <DropdownMenu open={filterMenuOpen} onOpenChange={setFilterMenuOpen}>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              aria-label="Filter by project"
              data-project-filter
              className={cn(
                "flex h-8 min-w-0 flex-1 items-center gap-2 rounded-[7px] px-2.5",
                "border border-transparent bg-transparent text-xs font-semibold text-foreground/80",
                "transition-colors duration-150 hover:border-border/60 hover:bg-foreground/[0.04]",
              )}
            >
              {filter === null ? (
                <Folder className="size-3.5 shrink-0 text-muted-foreground" />
              ) : (
                <ProjectMiniAvatar name={filterName ?? ""} path={filter} />
              )}
              <span className="min-w-0 flex-1 truncate text-left">
                {filter === null ? "All projects" : filterName}
              </span>
              <ChevronDown
                className={cn(
                  "size-3 shrink-0 text-muted-foreground transition-transform duration-150",
                  filterMenuOpen && "rotate-180",
                )}
              />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent side="bottom" align="start" className="w-[248px]">
            <DropdownMenuItem
              onClick={() => setFilter(null)}
              aria-label="All projects"
              className={cn(
                "h-8 gap-2 rounded-[7px] px-2 text-xs font-semibold",
                filter === null && "bg-foreground/[0.08] text-foreground",
              )}
            >
              <Folder className="size-3.5 shrink-0 text-muted-foreground" />
              <span className="min-w-0 flex-1 truncate">All projects</span>
              <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground">
                {projectCounts.total}
              </span>
            </DropdownMenuItem>
            {projectGroups.map((group) => (
              <ProjectFilterItem
                key={group.projectPath}
                name={group.projectName}
                path={group.projectPath}
                count={projectCounts.map.get(group.projectPath) ?? 0}
                active={filter === group.projectPath}
                onSelect={() => setFilter(group.projectPath)}
              />
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              aria-label="Add repository"
              className="flex size-8 shrink-0 items-center justify-center rounded-[7px] border border-dashed border-border text-sm leading-none text-muted-foreground transition-colors duration-150 hover:border-muted-foreground/60 hover:text-foreground"
            >
              +
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent side="bottom" align="start">
            <DropdownMenuItem onClick={() => openProject()} className="text-xs">
              <FolderOpen className="mr-2 h-3.5 w-3.5" />
              Open project
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={() => setShowNewProjectScreen(true)}
              className="text-xs"
            >
              <FolderPlus className="mr-2 h-3.5 w-3.5" />
              New project
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      <div className="px-2.5 pb-2.5" onContextMenuCapture={handleListContextMenuCapture}>
        {activeCards.length === 0 && filteredPending.length === 0 && (
          <div className="px-2 py-6 text-center text-xs text-muted-foreground">
            Nothing active
            {filterName && (
              <>
                {" in "}
                <span className="font-mono">{filterName}</span>
              </>
            )}
          </div>
        )}

        {topTier.map((ws, index) => renderCard(ws, index))}

        {filteredPending.map((pw) => (
          <div
            key={pw.id}
            className={cn(
              "flex items-center gap-2 px-2 py-2 text-sm",
              pw.status === "failed" ? "opacity-60" : "animate-pulse opacity-70",
            )}
          >
            {pw.status === "creating" ? (
              <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin text-muted-foreground" />
            ) : (
              <AlertCircle className="h-3.5 w-3.5 shrink-0 text-destructive" />
            )}
            <span className="truncate text-xs text-muted-foreground">
              {pw.status === "failed" ? pw.errorMessage || "Failed" : pw.name}
            </span>
          </div>
        ))}

        {/* The wind-down tier. Same full card, same actions — only its place
            in the list changed, and only while the rule above holds. No
            divider at all when nothing qualifies: an empty section header
            would imply a category the list isn't currently using. */}
        {wrappingUpTier.length > 0 && (
          <>
            <WrappingUpDivider />
            {wrappingUpTier.map((ws, index) =>
              renderCard(ws, topTier.length + index),
            )}
          </>
        )}

        {snoozedRows.length > 0 && (
          <>
            <ShelfHeader
              label="Snoozed"
              count={snoozedRows.length}
              showCount={snoozeCollapsed}
              collapsed={snoozeCollapsed}
              onToggle={() => setSnoozeCollapsed((c) => !c)}
            />
            {visibleSnoozed.map(({ entry, workspace }) => {
              const repo = repoByWorkspace.get(workspace.workspace_id);
              if (!repo) return null;
              return (
                <SnoozeRow
                  key={workspace.workspace_id}
                  workspace={workspace}
                  repo={repo}
                  isActive={workspace.workspace_id === activeWorkspaceId}
                  selected={selectedIds.has(workspace.workspace_id)}
                  status={
                    paneStatuses
                      ? getWorkspaceStatus(workspace.surfaces, paneStatuses)
                      : null
                  }
                  timeUntil={formatTimeUntil(entry.until - now)}
                  onWake={(id) => wake(id, "user")}
                  onSelect={handleSelect}
                  onMarkUnread={handleMarkUnread}
                />
              );
            })}
          </>
        )}

        {settledRows.length > 0 && (
          <>
            <ShelfHeader
              label="Settled"
              count={settledRows.length}
              showCount
              collapsed={settledCollapsed}
              onToggle={() => setSettledCollapsed((c) => !c)}
            />
            {visibleSettled.map(({ entry, workspace }) => {
              const repo = repoByWorkspace.get(workspace.workspace_id);
              if (!repo) return null;
              return (
                <SettledRow
                  key={workspace.workspace_id}
                  workspace={workspace}
                  repo={repo}
                  isActive={workspace.workspace_id === activeWorkspaceId}
                  selected={selectedIds.has(workspace.workspace_id)}
                  status={
                    paneStatuses
                      ? getWorkspaceStatus(workspace.surfaces, paneStatuses)
                      : null
                  }
                  time={formatElapsed(now - resolveSettledTimestamp(entry))}
                  justSettled={justSettledId === workspace.workspace_id}
                  onUnsettle={handleUnsettle}
                  onSelect={handleSelect}
                  onMarkUnread={handleMarkUnread}
                />
              );
            })}
            {!settledCollapsed &&
              settledHidden > 0 &&
              (() => {
                const next = Math.min(SETTLED_PAGE_COUNT, settledHidden);
                return (
                  <button
                    type="button"
                    data-settled-more
                    aria-label={`Show ${next} more settled workspaces (${settledHidden} hidden)`}
                    onClick={() =>
                      setSettledVisibleCount((c) => c + SETTLED_PAGE_COUNT)
                    }
                    className="flex h-7 w-full items-center justify-center rounded-lg font-mono text-[10.5px] text-muted-foreground/70 transition-colors duration-150 hover:text-foreground"
                  >
                    {`Show ${next} more (${settledHidden} hidden)`}
                  </button>
                );
              })()}
          </>
        )}
      </div>

      {/* Bulk right-click menu. Anchored to a zero-size element parked at the
          pointer, because the gesture belongs to the selection rather than to
          any one row's trigger. */}
      {bulkMenuAt !== null && (
        <DropdownMenu
          open
          onOpenChange={(open) => {
            if (!open) closeBulkMenu();
          }}
        >
          <DropdownMenuTrigger asChild>
            <span
              aria-hidden="true"
              data-bulk-anchor
              className="pointer-events-none fixed"
              style={{ left: bulkMenuAt.x, top: bulkMenuAt.y }}
            />
          </DropdownMenuTrigger>
          <DropdownMenuContent side="bottom" align="start">
            {/* Hidden, not disabled, when the selection contains live or
                blocked work — same treatment as Snooze below, so the two
                parking verbs can't drift apart. Since both verbs share one
                guardrail, hiding them empties the menu; the disabled line
                says why instead of leaving a blank popover. */}
            {!canBulkSettle && (
              <DropdownMenuItem disabled className="text-xs">
                Selection includes working or blocked workspaces
              </DropdownMenuItem>
            )}
            {canBulkSettle && (
              <DropdownMenuItem onClick={handleBulkSettle} className="text-xs">
                {`Settle (${selection.length})`}
              </DropdownMenuItem>
            )}
            {canBulkSnooze && (
              <DropdownMenuSub
                onOpenChange={(open) => {
                  if (open) setBulkSnoozePresets(computeSnoozePresets(Date.now()));
                }}
              >
                <DropdownMenuSubTrigger className="text-xs">
                  {`Snooze (${selection.length})`}
                </DropdownMenuSubTrigger>
                <DropdownMenuSubContent>
                  {bulkSnoozePresets.map((preset) => (
                    <DropdownMenuItem
                      key={preset.id}
                      onClick={() => handleBulkSnooze(preset.at)}
                      className="gap-4 text-xs"
                    >
                      <span className="flex-1">{preset.label}</span>
                      <span className="shrink-0 font-mono text-[10px] tabular-nums text-muted-foreground">
                        {preset.whenLabel}
                      </span>
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuSubContent>
              </DropdownMenuSub>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      )}
    </div>
  );
}
