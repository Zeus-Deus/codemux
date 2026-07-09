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
 *  at a glance. Mirrors Superset's color-coded badge style. */
const STATE_TO_TONE: Record<PrStatusState, string> = {
  merged: "text-accent-violet bg-accent-violet/15",
  open: "text-status-open bg-status-open/15",
  closed: "text-destructive bg-destructive/15",
  draft: "text-muted-foreground bg-muted-foreground/15",
};

/** Tinted border/fill/text per PR state for a bordered "PR chip" button
 *  (the workspace context bar's PR chip and the Context Row's status
 *  cluster). Same tone family as `STATE_TO_TONE` above, but with the
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
