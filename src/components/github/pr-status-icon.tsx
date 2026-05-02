import { GitMerge, GitPullRequest, GitPullRequestClosed, GitPullRequestDraft } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";

export type PrStatusState = "open" | "merged" | "closed" | "draft";

interface IconSpec {
  Icon: LucideIcon;
  colorCls: string;
}

const STATE_TO_ICON: Record<PrStatusState, IconSpec> = {
  merged: { Icon: GitMerge, colorCls: "text-purple-500" },
  open: { Icon: GitPullRequest, colorCls: "text-emerald-500" },
  closed: { Icon: GitPullRequestClosed, colorCls: "text-destructive" },
  draft: { Icon: GitPullRequestDraft, colorCls: "text-muted-foreground" },
};

/** Tinted background + text-color pair for the sidebar PR pill — matches
 *  the icon color at low opacity so the row reads "this PR is merged/open/…"
 *  at a glance. Mirrors Superset's color-coded badge style. */
const STATE_TO_TONE: Record<PrStatusState, string> = {
  merged: "text-purple-500 bg-purple-500/15",
  open: "text-emerald-500 bg-emerald-500/15",
  closed: "text-destructive bg-destructive/15",
  draft: "text-muted-foreground bg-muted-foreground/15",
};

export function prStatusToneClass(state: string | null | undefined): string | null {
  const normalized = normalizePrState(state);
  if (!normalized) return null;
  return STATE_TO_TONE[normalized];
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
