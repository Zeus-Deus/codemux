/**
 * Shared surface vocabulary for the review surfaces.
 *
 * Buttons here are borderless tinted fills only. A 1px outline reads as
 * a second border against the panel's own dividers at this density, and
 * the design canvas drops them everywhere — so the variants below are
 * the whole set, and `@/components/ui/button`'s `outline` variant is
 * deliberately not used on this surface.
 */

const BASE =
  "inline-flex h-7 shrink-0 items-center justify-center gap-1.5 rounded-md border-0 " +
  "px-2.5 text-[11px] font-medium transition-colors outline-none " +
  "focus-visible:ring-[1.5px] focus-visible:ring-ring/60";

/** Neutral secondary — sits on the bar's card background. */
export const btnCard = `${BASE} bg-card text-foreground/90 hover:bg-accent/50`;

/** Emphasized neutral (e.g. "Ready for review"). */
export const btnCardStrong = `${BASE} bg-card font-semibold text-foreground hover:bg-accent/50`;

/** Ember tint — agent handoffs and secondary primaries. */
export const btnEmber = `${BASE} bg-accent-ember/15 font-semibold text-accent-ember hover:bg-accent-ember/25`;

/** Ember solid — the one primary on a create/retry surface. */
export const btnEmberSolid = `${BASE} bg-accent-ember font-semibold text-[#1a1512] hover:bg-accent-ember/90`;

/** Green tint — Approve. A verdict, not a merge. */
export const btnGreenTint = `${BASE} bg-status-open/15 font-semibold text-status-open hover:bg-status-open/25`;

/** Green solid — Merge, and only Merge. */
export const btnGreenSolid = `${BASE} bg-status-open font-semibold text-[#0e1a12] hover:bg-status-open/90`;

/**
 * Merge while blocked. Same box as {@link btnGreenSolid} — same height,
 * same padding, same slot — because the reason it can't run lives in
 * words beside it, not in the button's geometry (binding rule 1).
 */
export const btnGreenMuted = `${BASE} bg-card text-muted-foreground`;

/** Bare text action (the strategy dropdown trigger). */
export const btnQuiet = `${BASE} bg-transparent text-muted-foreground hover:text-foreground`;

/** Relative age in the panel's voice: "38m ago", "3d ago", "just now". */
export function relativeAge(iso: string | null | undefined): string | null {
  if (!iso) return null;
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return null;
  const seconds = Math.max(0, Math.round((Date.now() - then) / 1000));
  if (seconds < 45) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${Math.max(1, minutes)}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.round(hours / 24);
  if (days < 30) return `${days}d ago`;
  return new Date(then).toLocaleDateString();
}

/** Compact age without the "ago" — for "Showing data from 4m ago". */
export function shortAge(ms: number): string {
  const seconds = Math.max(0, Math.round(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  return `${Math.round(minutes / 60)}h`;
}

/** `1,180` — diff stats are read as magnitudes, so they get separators. */
export function groupDigits(n: number): string {
  return n.toLocaleString("en-US");
}

/** A check's state in the three buckets this surface colours by. */
export type CheckState = "pass" | "fail" | "running" | "neutral";

export function checkState(conclusion: string | null, status: string): CheckState {
  const s = (conclusion ?? status).toLowerCase();
  if (s === "pass" || s === "success" || s === "neutral") return "pass";
  if (s === "fail" || s === "failure" || s === "error" || s === "timed_out") return "fail";
  if (s === "skipping" || s === "skipped" || s === "cancel" || s === "cancelled")
    return "neutral";
  return "running";
}

/** Plural helper that keeps the sentences in the bar readable. */
export function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}
