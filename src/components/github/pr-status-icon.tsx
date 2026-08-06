import { GitMerge, GitPullRequest, GitPullRequestClosed, GitPullRequestDraft } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";

export type PrStatusState = "open" | "merged" | "closed" | "draft";

interface IconSpec {
  Icon: LucideIcon;
  colorCls: string;
}

const STATE_TO_ICON: Record<PrStatusState, IconSpec> = {
  merged: { Icon: GitMerge, colorCls: "text-accent-violet" },
  open: { Icon: GitPullRequest, colorCls: "text-status-open" },
  closed: { Icon: GitPullRequestClosed, colorCls: "text-destructive" },
  draft: { Icon: GitPullRequestDraft, colorCls: "text-muted-foreground" },
};

/** Tinted background + text-color pair for the sidebar PR pill — matches
 *  the icon color at low opacity so the row reads "this PR is merged/open/…"
 *  at a glance. */
const STATE_TO_TONE: Record<PrStatusState, string> = {
  merged: "text-accent-violet bg-accent-violet/15",
  open: "text-status-open bg-status-open/15",
  closed: "text-destructive bg-destructive/15",
  draft: "text-muted-foreground bg-muted-foreground/15",
};

/** Tinted border/fill/text per PR state for the Context Row's bordered
 *  PR chip button. Same tone family as `STATE_TO_TONE` above, but with the
 *  border + hover treatment a clickable chip needs so the PR reads as a
 *  labeled action rather than a bare icon. Single home for both
 *  consumers — do not duplicate this map. */
export const PR_CHIP_TONE: Record<PrStatusState, string> = {
  open: "text-status-open border-status-open/40 bg-status-open/10 hover:bg-status-open/20",
  merged:
    "text-accent-violet border-accent-violet/40 bg-accent-violet/10 hover:bg-accent-violet/20",
  closed:
    "text-destructive border-destructive/40 bg-destructive/10 hover:bg-destructive/20",
  draft:
    "text-muted-foreground border-muted-foreground/40 bg-muted-foreground/10 hover:bg-muted-foreground/20",
};

/** Deferred-color variant of `STATE_TO_ICON` for the sidebar's parked rows.
 *  A settled row is history: it rests grayed out and only lights up under the
 *  pointer (or keyboard focus), so its PR badge starts neutral and picks the
 *  state color up on hover. Same colors as `STATE_TO_ICON` — only the timing
 *  differs. The variants name the settled row's own group (`group/settled` in
 *  `sidebar-inbox.tsx`) and are written out in full because Tailwind only
 *  sees class names that appear literally in the source. */
const STATE_TO_SETTLED_HOVER: Record<PrStatusState, string> = {
  merged:
    "group-hover/settled:text-accent-violet group-focus-within/settled:text-accent-violet",
  open: "group-hover/settled:text-status-open group-focus-within/settled:text-status-open",
  closed:
    "group-hover/settled:text-destructive group-focus-within/settled:text-destructive",
  draft:
    "group-hover/settled:text-muted-foreground group-focus-within/settled:text-muted-foreground",
};

export function prStatusSettledHoverClass(state: string | null | undefined): string | null {
  const normalized = normalizePrState(state);
  if (!normalized) return null;
  return STATE_TO_SETTLED_HOVER[normalized];
}

export function prStatusToneClass(state: string | null | undefined): string | null {
  const normalized = normalizePrState(state);
  if (!normalized) return null;
  return STATE_TO_TONE[normalized];
}

/** Text-only color for a PR state — no background/border, for a value
 *  rendered inline in a detail row (the Context Row popover's "Pull
 *  request" row). Same color-per-state as `STATE_TO_ICON`/`PR_CHIP_TONE`
 *  above, just without their chip chrome. */
export function prStatusTextClass(state: string | null | undefined): string | null {
  const normalized = normalizePrState(state);
  if (!normalized) return null;
  return STATE_TO_ICON[normalized].colorCls;
}

export function normalizePrState(state: string | null | undefined): PrStatusState | null {
  if (!state) return null;
  const lower = state.toLowerCase();
  if (lower === "open" || lower === "merged" || lower === "closed" || lower === "draft") {
    return lower;
  }
  return null;
}

/** Does a workspace's PR badge actually describe *this checkout's* work?
 *
 *  The backend also shows a badge for a PR opened from a branch the worktree
 *  merely visited recently — the case an agent creates when it branches off,
 *  pushes, opens a PR, and checks the worktree back (see the sidebar doc's
 *  "PR association" section). Surfacing that is right: the workspace visibly
 *  produced a pull request and the user should be able to reach it. Drawing
 *  *lifecycle* conclusions from it is not — the branch the user is standing on
 *  can still be full of uncommitted work, and that PR merging says nothing
 *  about it.
 *
 *  So the badge renders unconditionally, while every rule that moves a card on
 *  the strength of a PR — auto-settle, and the "Wrapping up" demotion — asks
 *  this first. It lives beside `normalizePrState` because both the sidebar and
 *  the hover card need it and neither may import the other.
 *
 *  Missing information on *either* side reads as a match. An absent head branch
 *  means "association predates the field" rather than "side branch", so the
 *  honest default for an unknown is the behaviour that shipped before; real
 *  fallback associations always carry one. An absent `gitBranch` is the same
 *  kind of unknown seen from the other end — `git_branch_info` reports no
 *  branch during a rebase, a bisect, or any detached HEAD, while the stored
 *  head branch survives — and comparing a known name against that unknown would
 *  read as "different branch" and silently un-associate a workspace from its
 *  own open PR for the length of the rebase. Not knowing which branch you are
 *  on is not evidence that the PR belongs to another one. */
export function isPrOnCurrentBranch(
  prHeadBranch: string | null | undefined,
  gitBranch: string | null | undefined,
): boolean {
  if (!prHeadBranch || !gitBranch) return true;
  return prHeadBranch === gitBranch;
}

interface Props {
  state: string | null | undefined;
  /** Size in Tailwind spacing units (1 = 4px). 3.5 = 14px (default). */
  size?: number;
  strokeWidth?: number;
  className?: string;
}

export function PrStatusIcon({ state, size = 3.5, strokeWidth = 1.75, className }: Props) {
  const normalized = normalizePrState(state);
  if (!normalized) return null;
  const { Icon, colorCls } = STATE_TO_ICON[normalized];
  return (
    <Icon
      size={size * 4}
      strokeWidth={strokeWidth}
      className={cn(colorCls, className)}
    />
  );
}

export function humanizePrState(state: string | null | undefined): string | null {
  const normalized = normalizePrState(state);
  if (!normalized) return null;
  return normalized.charAt(0).toUpperCase() + normalized.slice(1);
}
