import { memo, useEffect, useState } from "react";
import {
  AlarmClock,
  Check,
  CircleDotDashed,
  Cloud,
  Pin,
  PinOff,
  Terminal,
} from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { cn } from "@/lib/utils";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ProviderLogo } from "@/components/chat/provider-logo";
import { IssueDetailPopover } from "@/components/github/issue-detail-popover";
import {
  PrStatusIcon,
  normalizePrState,
  prStatusTextClass,
} from "@/components/github/pr-status-icon";
import { WorkspaceInboxMenu } from "./workspace-inbox-menu";
import { WorkspaceHoverCard } from "./workspace-hover-card";
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";
import {
  useSidebarDensityStore,
  formatElapsed,
  permissionBlockerText,
} from "@/stores/sidebar-density-store";
import { useProjectAppearance } from "./use-project-appearance";
import { isRowActivationKey } from "./sidebar-row-activation";
import { getWorkspaceProviders } from "@/lib/pane-status";
import { computeSnoozePresets, type SnoozePreset } from "./sidebar-snooze";
import type { ActivePaneStatus, WorkspaceSnapshot } from "@/tauri/types";
import { providerForWorkspace, providerRef } from "@/lib/source-control";

export interface InboxRepo {
  name: string;
  path: string;
}

/** Floor for the meta line's trailing cluster: the width of the *widest*
 *  single indicator (the 15px notification pill, not the 13px provider logo),
 *  so a card showing nothing there still can't pull its PR chip out past its
 *  neighbours'. */
export const META_CLUSTER_MIN_WIDTH = 15;

const PROVIDER_MARK_WIDTH = 13;
const RUN_INDICATOR_WIDTH = 12;
/** The cluster's own `gap-2`. */
const META_CLUSTER_GAP = 8;

/** How much room the trailing indicator cluster should reserve for one list
 *  render, given the largest number of provider marks any visible card carries
 *  and whether *any* of them shows the running-process indicator.
 *
 *  Reserving per list rather than per card is the whole point: the PR chip is
 *  right-aligned against this cluster, so a card with one provider mark and a
 *  card with three would otherwise start their chips at different x positions
 *  and the column would read as ragged noise down a scrolling list. Reserving
 *  only what *this* list actually needs — rather than a fixed worst case —
 *  means a sidebar where nothing is running and everything is single-provider
 *  doesn't pay a permanent indent for indicators no card shows.
 *
 *  Applied as a `min-width`, not a fixed width, because unlike the source
 *  design this cluster can also carry a remote-host icon and a notification
 *  pill; those are per-card and rare, so they are allowed to push past the
 *  reservation rather than force every card to pay for them. */
export function metaClusterWidth(
  maxProviderMarks: number,
  anyRunIndicator: boolean,
): number {
  const marks = Math.max(0, maxProviderMarks);
  const width =
    marks * PROVIDER_MARK_WIDTH +
    Math.max(0, marks - 1) * META_CLUSTER_GAP +
    (anyRunIndicator
      ? RUN_INDICATOR_WIDTH + (marks > 0 ? META_CLUSTER_GAP : 0)
      : 0);
  return Math.max(META_CLUSTER_MIN_WIDTH, width);
}

interface Props {
  workspace: WorkspaceSnapshot;
  repo: InboxRepo;
  isActive: boolean;
  status: ActivePaneStatus | null;
  /** Settings → Appearance → Sidebar → Show git stats. Off hides the ↑ahead
   *  and +/− numbers; the branch name stays. */
  showGitStats: boolean;
  /** Coarse (~30s) clock from the parent — one interval for the whole list. */
  now: number;
  /** True while the settle animation is collapsing this card (~200ms before
   *  it re-renders as a settled row). */
  leaving: boolean;
  /** True briefly after an un-settle so the returning card eases back in. */
  justUnsettled: boolean;
  /** 1-9 digit shown as an overlay badge while the jump modifier is held, or
   *  null when no hint should show. Rendered as an overlay so it never shifts
   *  the card layout. */
  jumpHint?: number | null;
  onSettle: (workspaceId: string) => void;
  onSnooze: (workspaceId: string, until: number) => void;
  /** A pinned card stays above every lifecycle tier. */
  pinned?: boolean;
  onUnpin?: (workspaceId: string) => void;
  /** Pin from the card itself. Pinning used to be a context-menu-only gesture,
   *  which made the one action that reorders the list the hardest to reach. */
  onPin?: (workspaceId: string) => void;
  /** Port of a long-running process detected in this workspace (a dev server,
   *  a watcher), or null when nothing is listening. Passed down as a plain
   *  number rather than resolved per card so the whole inbox holds exactly one
   *  subscription to the ports domain — see `sidebar-inbox.tsx`. */
  runningPort?: number | null;
  /** Reserved width (px) for the meta line's trailing indicator cluster,
   *  computed once per list render from the widest cluster any visible card
   *  needs. Without it the PR chip starts at a different x on every card and
   *  the column reads as ragged. */
  metaClusterMinWidth?: number;
  /** The agent finished here since the user last opened this workspace. */
  unread: boolean;
  /** Recently returned from a snooze. The list order is static, so a woken
   *  card reappears in its old position — this badge is what carries the
   *  signal that it is back. */
  woke: boolean;
  selected: boolean;
  onSelect: (workspaceId: string, mode: "single" | "toggle" | "range") => void;
  onMarkUnread: (workspaceId: string) => void;
}

/** One active-workspace card in the flat sidebar inbox: repo eyebrow, work
 *  title + issue chip, optional blocker line (needs-you only), and a mono
 *  meta line, left-to-right: branch · ↑ahead · +/− — then, right-aligned,
 *  PR chip · provider marks / remote / notifications.
 *  The right side of the eyebrow shows the agent state, swapping to a
 *  "✓ Settle" button on hover/focus.
 *
 *  Memoized: at real profile scale the inbox holds dozens of these, and the
 *  steady state is one workspace's metadata moving (a git sweep tick, an agent
 *  status flip). The parent hands over primitives plus references that survive
 *  a backend tick — the workspace object itself (structural sharing on
 *  snapshots, targeted replacement on deltas), an interned `repo`, and
 *  `useCallback`'d handlers — so this boundary actually bails out instead of
 *  paying for a comparison that always fails.
 */
export const SidebarInboxCard = memo(function SidebarInboxCard({
  workspace,
  repo,
  isActive,
  status,
  showGitStats,
  now,
  leaving,
  justUnsettled,
  jumpHint,
  onSettle,
  onSnooze,
  pinned = false,
  onUnpin,
  onPin,
  runningPort = null,
  metaClusterMinWidth = META_CLUSTER_MIN_WIDTH,
  unread,
  woke,
  selected,
  onSelect,
  onMarkUnread,
}: Props) {
  const appearance = useProjectAppearance(repo.path);

  // Observe status transitions so elapsed labels (idle "26m") can be derived
  // client-side — the backend stamps no status-changed-at.
  const observeStatus = useSidebarDensityStore((s) => s.observeStatus);
  useEffect(() => {
    observeStatus(workspace.workspace_id, status);
  }, [observeStatus, workspace.workspace_id, status]);
  const settledAt = useSidebarDensityStore(
    (s) => s.settledAt[workspace.workspace_id],
  );

  // Radix portals the open Snooze menu out of this card, so once it opens the
  // pointer is no longer over the card and focus no longer sits inside it —
  // the CSS-only hover/focus swap below would hide the trigger mid-interaction
  // and Radix would close the menu it is anchored to. Tracking the open state
  // lets us pin the action cluster on for exactly as long as the menu lives.
  const [snoozeMenuOpen, setSnoozeMenuOpen] = useState(false);

  // Wake times are resolved the moment the menu opens, never while the list
  // renders. Sharing the inbox's coarse (~30s) clock made "In 1 hour" mean "an
  // hour from up to half a minute ago" by the time it was clicked, and it made
  // every card in the sidebar re-render on every tick to keep a menu fresh that
  // nobody had opened — the exact cost that shows up with many workspaces.
  const [snoozePresets, setSnoozePresets] = useState<SnoozePreset[]>([]);
  const handleSnoozeMenuOpenChange = (open: boolean) => {
    if (open) setSnoozePresets(computeSnoozePresets(Date.now()));
    setSnoozeMenuOpen(open);
  };

  const handleActivate = () => {
    activateWorkspaceInteraction(workspace.workspace_id).catch(console.error);
  };

  /** A plain activation (click, Enter, Space) also collapses any multi-select
   *  down to this card — otherwise navigating away would leave an invisible
   *  selection behind that the next bulk action would silently act on. */
  const selectAndActivate = () => {
    onSelect(workspace.workspace_id, "single");
    handleActivate();
  };

  const handleClick = (e: React.MouseEvent) => {
    const mode = e.metaKey || e.ctrlKey ? "toggle" : e.shiftKey ? "range" : null;
    if (mode) {
      // A modified click is a selection gesture, not navigation: activating
      // here would yank the user into a workspace they were only ticking off
      // for a bulk action. preventDefault also keeps the shift-click from
      // painting a text selection across the cards it spans.
      e.preventDefault();
      e.stopPropagation();
      onSelect(workspace.workspace_id, mode);
      return;
    }
    selectAndActivate();
  };

  const isRemote = workspace.host_id !== null && workspace.host_id !== undefined;

  const isWorking = status === "working";
  const isNeeds = status === "permission";
  const isMonitoring = status === "monitoring";
  const isDone = status === "review";

  // Background recede. Prominence is a scarce resource, so it is reserved for
  // the rows that actually want a human right now: the workspace you are in,
  // one that is blocked on you, one that has finished and wants a review, one
  // holding output you have not read, and one that just came back from a
  // snooze — plus anything you have ticked for a bulk action. Everything else
  // is either an agent quietly working (not your problem yet), an agent
  // babysitting something in the background, or an idle row you have already
  // read, so it sits back at reduced opacity and the ones that need you read
  // as the bright rows in the list. Hovering or focusing a receded card
  // restores it in full, so nothing is ever hidden — only ranked.
  const receded =
    !isActive &&
    !selected &&
    !unread &&
    !woke &&
    (status === "working" || status === "monitoring" || status === null);

  // Settle safety net: a live or blocked agent can never be swept out of
  // sight. Finished ("review"), monitoring and idle cards all offer Settle —
  // sweeping completed work aside is the whole point of the gesture, and a
  // workspace left babysitting CI is exactly the kind of thing a user parks.
  // Pinned cards are exempt: the pin is a visibility override that parking
  // would silently defeat.
  const canSettle =
    !pinned && status !== "working" && status !== "permission";

  // Snooze hides the card just as thoroughly as Settle does, so it rides the
  // same guardrail rather than restating it — a second copy of the condition
  // is exactly how a later edit to one of them quietly opens a hole that lets
  // live or blocked work be buried. This is now the *only* thing gating the
  // Snooze trigger: the preset list no longer doubles as an offered/not-offered
  // signal, because it does not exist until the menu opens.
  const canSnooze = canSettle;

  // While an agent is live and a linked issue exists, the card title IS the
  // work (issue title); the worktree name stays reachable via the branch on
  // the meta line + the hover card. Idle cards keep the workspace name.
  const displayTitle =
    status !== null && workspace.linked_issue
      ? workspace.linked_issue.title
      : workspace.title;

  const prState = normalizePrState(workspace.pr_state);
  // `scProvider` — the *hosting* product. Distinct from `providers`
  // above, which is the set of AI agent backends running in this
  // workspace's panes.
  const scProvider = providerForWorkspace(workspace);
  const hasAhead = showGitStats && workspace.git_ahead > 0;
  const hasStats =
    showGitStats &&
    (workspace.git_additions > 0 || workspace.git_deletions > 0);
  const idleTime =
    settledAt != null ? formatElapsed(now - settledAt) : null;

  const handlePrClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (workspace.pr_url) openUrl(workspace.pr_url).catch(console.error);
  };

  // The official mark of each agent provider chatting in this workspace
  // (Claude / Codex / OpenCode), shown on the meta line's right cluster.
  const providers = getWorkspaceProviders(workspace.surfaces);

  // With the Snooze menu open the swap can no longer be driven by hover/focus
  // (both have left the card for the portal), so it is driven by state
  // instead — the actions stay up and the state cluster stays down until the
  // menu closes and Radix returns focus to the trigger inside the card.
  const actionsPinned = snoozeMenuOpen;

  /** Shared shape for the eyebrow's hover-revealed actions: borderless,
   *  transparent, one muted ink that resolves to full foreground on the
   *  button's own hover. The actions used to be bordered pills, which gave
   *  three fills and three outlines to a strip that is meant to read as chrome
   *  until you aim at it — and made a hovered card look busier than a card
   *  with a live agent. Bare glyphs put the weight back on the card's content;
   *  Settle keeps its word because it is the one destructive-feeling verb here
   *  and a check glyph alone doesn't name it. */
  const eyebrowGlyphClass = cn(
    "flex shrink-0 items-center border-none bg-transparent p-0",
    "text-muted-foreground/75 transition-colors duration-150 hover:text-foreground",
  );

  const stateCluster = (
    <span
      className={cn(
        "flex shrink-0 items-center gap-1.5 text-[11px]",
        // The hover/focus swap: state hides, Snooze/Settle show. CSS-only so
        // no re-render churn on pointer moves. The state yields to the *wide*
        // actions only — a card that can't settle reveals nothing but the Pin
        // glyph, which is narrow enough to sit beside the readout, so its
        // state keeps its place. That is the point of the guardrail: a live,
        // blocked or pinned card must never trade its status for chrome the
        // user can't use anyway.
        canSettle &&
          (actionsPinned
            ? "hidden"
            : "group-hover/card:hidden group-focus-within/card:hidden"),
        isWorking && "font-semibold text-status-working",
        isNeeds && "font-semibold text-status-attention",
        isMonitoring && "font-semibold text-status-monitoring",
        isDone && "font-semibold text-status-open",
        !status && "font-medium text-muted-foreground/70",
      )}
    >
      {/* Workspace navigation reports lifecycle, not the agent's current
          action. Keep its working mark static; the animated activity orb
          belongs inside the thread where that activity has context. */}
      {isWorking && (
        <CircleDotDashed
          data-workspace-working-icon
          aria-hidden
          className="size-3.5"
          strokeWidth={2}
        />
      )}
      {isNeeds && (
        <span className="size-1.5 animate-pulse rounded-full bg-status-attention" />
      )}
      {/* Steady dot, deliberately not the configurable WorkingIndicator and
          deliberately not animated: monitoring is calm background presence. */}
      {isMonitoring && (
        <span className="size-1.5 rounded-full bg-status-monitoring" />
      )}
      {isDone && <Check className="h-3 w-3" strokeWidth={2.5} />}
      {isWorking
        ? "Working"
        : isNeeds
          ? "Needs you"
          : isMonitoring
            ? "Monitoring"
            : isDone
              ? "Done · review"
              : idleTime}
    </span>
  );

  return (
    <WorkspaceInboxMenu
      workspace={workspace}
      repo={repo}
      settleAction={
        canSettle && !leaving
          ? { kind: "settle", onAction: () => onSettle(workspace.workspace_id) }
          : undefined
      }
      snoozeAction={
        canSnooze && !leaving
          ? {
              kind: "snooze",
              offered: true,
              onSnooze: (until) => onSnooze(workspace.workspace_id, until),
              // An active card can only defer, never wake — waking belongs to
              // the snoozed row shape, which passes kind: "wake" instead.
              onWake: () => {},
            }
          : undefined
      }
      unreadAction={
        // Offering "Mark unread" on a card that already reads as unread would
        // be a menu entry that visibly does nothing.
        unread
          ? undefined
          : { onMarkUnread: () => onMarkUnread(workspace.workspace_id) }
      }
    >
      <div
        className={cn(
          "overflow-hidden transition-[max-height,opacity] duration-200 ease-out",
          // Off-screen cards stop costing layout and paint. At 50+ workspaces
          // the inbox is several viewports tall and every card was laying out
          // and painting on each coarse tick even when nowhere near the
          // scrollport. `content-visibility: auto` skips exactly that work and
          // nothing else.
          //
          // Deliberately NOT a virtualizer: these rows stay mounted, so in-page
          // find, focus and tab order, the Alt+1..9 jump targets, the forced
          // "you are here" row, and every per-row effect keep working. A
          // virtualizer would break all of those to win nothing on a list that
          // is normally well under a few hundred rows.
          //
          // The intrinsic size is the wrapper's real resting height (card box +
          // its 6px bottom margin, measured in the running app: 86.7px for a
          // plain card, 89.4px with a PR chip). A hint that is wrong in either
          // direction makes the scrollbar jump as rows are realised, so it is
          // set at the median rather than the extremes; the `auto` keyword then
          // replaces it with the row's true size once it has been rendered
          // once.
          "[content-visibility:auto] [contain-intrinsic-size:auto_88px]",
          leaving ? "max-h-0 opacity-0" : "max-h-40 opacity-100",
          justUnsettled && "rise-in",
        )}
      >
        {/* Hover details live on the inner card, not on the animation wrapper
            above — that wrapper is already the ContextMenuTrigger, and two
            `asChild` triggers must not compose onto the same node. */}
        <WorkspaceHoverCard workspace={workspace} repo={repo} status={status}>
            <div
              role="button"
              tabIndex={0}
              data-inbox-card={workspace.workspace_id}
              data-selected={selected || undefined}
              onClick={handleClick}
              onKeyDown={(e) => {
                // Same guard the settled and snoozed rows use: this card hosts
                // its own buttons (PR chip, snooze menu), and a key press on
                // one of those bubbles up here. Without the target check, Enter
                // on the PR chip would open the PR *and* yank the main pane
                // onto the workspace, and Space would never reach the button at
                // all — the preventDefault below would eat its native click.
                if (!isRowActivationKey(e)) return;
                // Space would otherwise scroll the sidebar as it activates
                // the card.
                if (e.key === " ") e.preventDefault();
                selectAndActivate();
              }}
              className={cn(
                "group/card relative mb-1.5 cursor-pointer rounded-[10px] border px-[11px] pt-[9px] pb-[10px]",
                // select-none: a shift-click range gesture would otherwise
                // drag a text highlight across every card it spans.
                "select-none outline-none duration-150",
                // The opacity axis lives on this node, not on the animation
                // wrapper above — that wrapper already owns one (the leaving
                // collapse and the rise-in keyframe both drive opacity), and a
                // second declaration there would fight them.
                "transition-[color,background-color,border-color,opacity]",
                isActive
                  ? "border-border bg-foreground/[0.09]"
                  : isNeeds
                    ? "border-status-attention/30 bg-transparent hover:bg-foreground/[0.05]"
                    : "border-transparent bg-transparent hover:bg-foreground/[0.05] focus-visible:border-border",
                // Multi-select layers a ring over whatever the card already
                // is, so "checked for a bulk action" never has to compete with
                // "this is the workspace you're looking at" for the same
                // pixels. Ember is the one accent reserved for selection; the
                // resting/active treatments stay pure lightness.
                selected &&
                  "border-transparent bg-accent-ember/[0.08] ring-1 ring-accent-ember/55",
                // Hover restores by pointer, focus-within by keyboard. The
                // pinned exception is the Snooze dropdown: Radix portals the
                // open menu out of this card, so the pointer has left and
                // focus sits outside — neither restore would hold, and the
                // card the user is actively operating on would dim mid-menu.
                receded &&
                  !actionsPinned &&
                  "opacity-70 hover:opacity-100 focus-within:opacity-100",
              )}
            >
              {/* Jump-shortcut hint: the digit that activates this card while
                  the jump modifier is held. Absolutely positioned so it
                  overlays the top-right corner without shifting layout. */}
              {jumpHint != null && (
                <span
                  aria-hidden="true"
                  className={cn(
                    // Hangs off the card's corner so it never covers the
                    // state label at the row's right edge.
                    "absolute -right-1 -top-1 z-10 flex h-4 min-w-4 items-center justify-center",
                    "rounded border border-border bg-muted px-1",
                    "font-mono text-[9px] text-muted-foreground",
                  )}
                >
                  {jumpHint}
                </span>
              )}

              {/* Eyebrow: repo identity + agent state / Settle swap */}
              <div className="flex min-h-5 items-center gap-1.5">
                <ProjectAvatar
                  name={repo.name}
                  color={appearance.customColor}
                  imageUrl={appearance.imageUrl}
                  cacheBust={appearance.imageVersion}
                  size="sm"
                  shape="square"
                  className="shrink-0 font-bold"
                />
                <span className="min-w-0 truncate text-[11px] font-semibold tracking-[0.01em] text-muted-foreground/80">
                  {repo.name}
                </span>
                {/* Resting pin marker. It sits beside the repo name rather
                    than at the row's right edge because that edge belongs to
                    the state readout, and it hides under the pointer: the
                    action cluster's own pin glyph occupies the same claim
                    while revealed, and showing both would read as two pins. */}
                {pinned && (
                  <Pin
                    role="img"
                    aria-label="Pinned workspace"
                    className={cn(
                      "size-[11px] shrink-0 text-muted-foreground/75",
                      actionsPinned
                        ? "hidden"
                        : "group-hover/card:hidden group-focus-within/card:hidden",
                    )}
                  />
                )}
                <span className="flex-1" />
                {/* "Woke": the list keeps a stable order, so a card returning
                    from a snooze slots back where it was and nothing about its
                    position says it moved. This pill is the only signal. Green
                    rather than red — it came back on schedule, it is not a
                    problem. Everything here is shrink-0 and the repo name
                    truncates, so a woke + hovered card squeezes the name
                    instead of pushing the actions off the card. */}
                {woke && (
                  <span
                    role="img"
                    aria-label={`"${workspace.title}" woke from snooze`}
                    title="Returned from snooze"
                    className={cn(
                      "flex shrink-0 items-center gap-0.5 rounded-full px-1.5",
                      "border border-status-open/25 bg-status-open/10",
                      "text-[10px] font-semibold leading-[15px] text-status-open",
                    )}
                  >
                    <AlarmClock className="h-2.5 w-2.5" strokeWidth={2.5} />
                    Woke
                  </span>
                )}
                {/* Hover/focus-revealed action cluster. Pin is unconditional —
                    it is the one action with no guardrail, and burying the
                    gesture that reorders the list in a context menu made it
                    the least discoverable thing the card can do. Snooze and
                    Settle ride the settle guardrail together, so a live or
                    blocked agent reveals a pin and nothing else. */}
                <span
                  className={cn(
                    "row-in shrink-0 items-center gap-[7px]",
                    actionsPinned
                      ? "flex"
                      : "hidden group-hover/card:flex group-focus-within/card:flex",
                  )}
                >
                  <button
                    type="button"
                    onClick={(e) => {
                      // Every action here stops propagation: these buttons sit
                      // inside the card's own click target, and without this a
                      // pin or a snooze would also yank the main pane onto the
                      // workspace the user was only filing away.
                      e.stopPropagation();
                      if (pinned) onUnpin?.(workspace.workspace_id);
                      else onPin?.(workspace.workspace_id);
                    }}
                    aria-label={
                      pinned
                        ? `Unpin "${workspace.title}"`
                        : `Pin "${workspace.title}" to top`
                    }
                    title={pinned ? "Unpin" : "Pin to top"}
                    className={eyebrowGlyphClass}
                  >
                    {pinned ? (
                      <PinOff className="size-3" strokeWidth={1.5} />
                    ) : (
                      <Pin className="size-3" strokeWidth={1.5} />
                    )}
                  </button>
                {canSnooze && (
                  <DropdownMenu
                    open={snoozeMenuOpen}
                    onOpenChange={handleSnoozeMenuOpenChange}
                  >
                    <DropdownMenuTrigger asChild>
                      <button
                        type="button"
                        onClick={(e) => e.stopPropagation()}
                        aria-label={`Snooze "${workspace.title}"`}
                        title="Snooze"
                        className={eyebrowGlyphClass}
                      >
                        <AlarmClock className="size-3" strokeWidth={1.5} />
                      </button>
                    </DropdownMenuTrigger>
                    {/* Radix portals this into document.body, but React events
                        still bubble along the *component* tree — without these
                        stoppers every preset click and every Enter inside the
                        menu would also hit the card's activate handlers and
                        navigate away while snoozing. The width override undoes
                        the default "match the trigger", which here is a 60px
                        button. */}
                    <DropdownMenuContent
                      align="end"
                      className="w-auto min-w-[132px]"
                      onClick={(e) => e.stopPropagation()}
                      onKeyDown={(e) => e.stopPropagation()}
                    >
                      {snoozePresets.map((preset) => (
                        <DropdownMenuItem
                          key={preset.id}
                          className="gap-4 text-xs"
                          onSelect={() =>
                            onSnooze(workspace.workspace_id, preset.at)
                          }
                        >
                          <span className="flex-1">{preset.label}</span>
                          {/* The relative label says how far, never when.
                              "Next week" without this is a deferral the user
                              has to guess the length of. */}
                          <span className="shrink-0 font-mono text-[10px] tabular-nums text-muted-foreground">
                            {preset.whenLabel}
                          </span>
                        </DropdownMenuItem>
                      ))}
                    </DropdownMenuContent>
                  </DropdownMenu>
                )}
                  {canSettle && (
                    <button
                      type="button"
                      onClick={(e) => {
                        e.stopPropagation();
                        onSettle(workspace.workspace_id);
                      }}
                      aria-label={`Settle "${workspace.title}"`}
                      className={cn(
                        // Settle keeps its word where Pin and Snooze went bare:
                        // it is the primary action of the whole inbox and the
                        // only one whose glyph (a check) reads as a claim about
                        // the work rather than a verb the user is performing.
                        "flex shrink-0 items-center gap-1 border-none bg-transparent p-0",
                        "text-[11px] font-semibold text-muted-foreground",
                        "transition-colors duration-150 hover:text-foreground",
                      )}
                    >
                      <Check className="size-[11px]" strokeWidth={2.1} />
                      Settle
                    </button>
                  )}
                </span>
                {stateCluster}
              </div>

              {/* Title line: work title + linked-issue chip */}
              <div className="mt-1 flex min-w-0 items-center gap-1.5">
                {/* Unread dot. Deliberately ember and deliberately NOT on the
                    eyebrow: "the agent finished and you haven't looked" is a
                    different claim from "Done · review" (which a card keeps
                    long after it has been read), so it must not borrow the
                    green those states own, and it must survive a hover that
                    swaps the eyebrow's right side out. */}
                {unread && (
                  <span
                    role="img"
                    aria-label={`Unread — "${workspace.title}"`}
                    title="New agent output since you last opened this workspace"
                    className="size-1.5 shrink-0 rounded-full bg-accent-ember"
                  />
                )}
                <span
                  className={cn(
                    "truncate text-[13px] leading-[1.35] text-foreground",
                    // The extra weight is what makes an unread card readable
                    // as unread at a glance down a scrolling list, where a
                    // 6px dot alone is easy to sweep past.
                    unread ? "font-bold" : "font-semibold",
                  )}
                >
                  {displayTitle}
                </span>
                {workspace.linked_issue && (
                  <IssueDetailPopover
                    providerKind={workspace.provider_kind}
                    workspaceId={workspace.workspace_id}
                    issue={workspace.linked_issue}
                  />
                )}
              </div>

              {/* Blocker line — needs-you cards only */}
              {isNeeds && (
                <div className="mt-0.5 truncate text-[11px] text-status-attention">
                  {permissionBlockerText(workspace)}
                </div>
              )}

              {/* Mono meta line, two columns: the git-local facts (branch ·
                  ↑ahead · +/−) flow from the left, then a flex spacer pins the
                  PR chip and the trailing indicator cluster to the right.
                  The spacer sits *before* the PR chip on purpose: with it after,
                  the chip started wherever the branch name happened to end, so
                  chips landed at a different x on every card and the column read
                  as ragged noise down a scrolling list. Right-aligning also makes
                  the chip's own variable width (#7 vs #1234) harmless.
                  The chip is deliberately chrome-free — state-colored icon +
                  number, no border or fill — so an active card and a settled
                  row draw one PR with the same geometry and the same treatment:
                  settling changes what the badge *is* only by deferring, never
                  by swapping shape. The settled row parks its state color on
                  hover (the whole row recedes at rest, see
                  `prStatusSettledHoverClass`), so at rest the settled copy is
                  the grey version of this chip and under the pointer it is
                  this chip exactly. */}
              <div
                data-meta-line
                className="mt-[5px] flex min-w-0 items-center gap-2 font-mono text-[11px] leading-tight text-muted-foreground/60"
              >
                {workspace.git_branch && (
                  <span className="min-w-0 truncate">{workspace.git_branch}</span>
                )}
                {hasAhead && (
                  <span className="shrink-0 tabular-nums">
                    ↑{workspace.git_ahead}
                  </span>
                )}
                {hasStats && (
                  <span className="shrink-0 tabular-nums">
                    {workspace.git_additions > 0 && (
                      <span className="text-status-open/80">
                        +{workspace.git_additions}
                      </span>
                    )}{" "}
                    {workspace.git_deletions > 0 && (
                      <span className="text-status-attention/80">
                        −{workspace.git_deletions}
                      </span>
                    )}
                  </span>
                )}
                <span className="flex-1" />
                {prState && (
                  <button
                    type="button"
                    onClick={handlePrClick}
                    disabled={!workspace.pr_url}
                    aria-label={
                      workspace.pr_number
                        ? `${scProvider.nounTitle} ${providerRef(scProvider, workspace.pr_number)} — ${prState}`
                        : `${scProvider.nounTitle} — ${prState}`
                    }
                    className={cn(
                      "inline-flex shrink-0 items-center gap-1 rounded px-1 py-px font-mono text-[10px] font-medium",
                      "transition-colors duration-150",
                      prStatusTextClass(prState),
                      workspace.pr_url
                        ? "hover:bg-foreground/[0.055]"
                        : // `cursor-default`, not `pointer-events-none`: the
                          // chip is already `disabled`, so it swallows the
                          // click. Letting pointer events pass through instead
                          // would hand the click to the card root and select
                          // the workspace, and the cursor would still read as
                          // the card's `cursor-pointer`.
                          "cursor-default opacity-65",
                    )}
                  >
                    <PrStatusIcon state={prState} size={3} className="shrink-0" />
                    {workspace.pr_number != null && (
                      <span>{providerRef(scProvider, workspace.pr_number)}</span>
                    )}
                  </button>
                )}
                {/* Trailing indicators keep the far-right column. The reserved
                    min-width is what stops a card with fewer indicators from
                    pulling its PR chip out past its neighbours' — the icon
                    cluster is the anchor the chip aligns against. The parent
                    sizes it once per list render from the widest cluster any
                    visible card needs (see `metaClusterWidth`), so the column
                    is stable across cards without every sidebar paying for
                    indicators none of its cards show. */}
                <span
                  className="flex shrink-0 items-center justify-end gap-2"
                  style={{ minWidth: `${metaClusterMinWidth}px` }}
                >
                  {/* Long-running process. Distinct from the agent-state
                      readout on the eyebrow: that says whether an *agent* is
                      doing something, this says a dev server or watcher the
                      user started is still holding a port — the thing that is
                      easy to forget about and expensive to leave running. It
                      pulses because a port is live state, and it takes the
                      same green as an open PR because both mean "up". */}
                  {runningPort != null && (
                    <span
                      role="img"
                      aria-label={`Long-running process on :${runningPort}`}
                      title={`Long-running process on :${runningPort}`}
                      className="flex shrink-0 animate-pulse items-center text-status-open"
                    >
                      <Terminal className="size-3" strokeWidth={1.7} />
                    </span>
                  )}
                  {providers.map((p) => (
                    <ProviderLogo
                      key={p}
                      provider={p}
                      className="h-[13px] w-[13px] opacity-80"
                    />
                  ))}
                  {isRemote && (
                    <Cloud
                      aria-label="Runs on a remote host"
                      className="h-[13px] w-[13px] shrink-0 text-status-remote"
                    />
                  )}
                  {workspace.notification_count > 0 && (
                    <span className="flex h-[15px] min-w-[15px] shrink-0 items-center justify-center rounded-full bg-foreground/10 px-1 text-[10px] font-bold text-muted-foreground">
                      {workspace.notification_count}
                    </span>
                  )}
                </span>
              </div>
            </div>
        </WorkspaceHoverCard>
      </div>
    </WorkspaceInboxMenu>
  );
});
