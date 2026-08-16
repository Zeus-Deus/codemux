/**
 * Shared surface vocabulary for the review surfaces.
 *
 * Buttons here are borderless tinted fills only. A 1px outline reads as
 * a second border against the panel's own dividers at this density, and
 * the design canvas drops them everywhere — so the variants below are
 * the whole set, and `@/components/ui/button`'s `outline` variant is
 * deliberately not used on this surface.
 */

/**
 * The type scale for the review surfaces — one ladder, one place.
 *
 * The design canvas was drawn compact, and read literally it lands a
 * whole step below the rest of the app: the sidebar, automations and the
 * composer all sit on 11–13px with 12–12.5px doing the primary work.
 * These tokens raise the floor to that ladder while keeping the canvas's
 * *relative* order intact — eyebrow < meta < body < title, everywhere.
 *
 * Callers keep their own weight, colour and tracking; only the size (and
 * for code, the line-height it can't be read without) lives here.
 */

/** Uppercase eyebrows and mono micro-labels. The floor — nothing below. */
export const tzEyebrow = "text-[11px]";
/** Meta, captions, hints — the quiet line under a title. */
export const tzMeta = "text-[12px]";
/** Secondary meta and mono numbers, a hair up from {@link tzMeta}. */
export const tzMetaNum = "text-[12px]";
/** Buttons, body-small, bar sentences. */
export const tzBody = "text-[12.5px]";
/** Body copy and row meta that has to be read, not scanned. */
export const tzBodyLg = "text-[13px]";
/** List/row titles. */
export const tzRowTitle = "text-[13px]";
/** Section and panel headers. */
export const tzPanelHeader = "text-[13.5px]";
/** The PR title in a panel. */
export const tzPanelTitle = "text-[15px]";
/** The PR title on the full-screen page, where there is room for it. */
export const tzPageTitle = "text-[17px]";
/** Check-log excerpts — mono, quieter than the diff it explains. */
export const tzLog = "text-[12px]";

/*
 * Deliberately no token for the diff body.
 *
 * Code in this app is sized by the user, not by us: `.code-surface`
 * reads `--font-size-code` from settings, and a review that overrode it
 * would be the one place in Codemux where the diff didn't match the
 * editor. What review *does* change is the leading and the gutter, and
 * those live in the diff views behind their `flow` prop so the Changes
 * pane keeps its tighter density.
 */

const BASE =
  "inline-flex h-[30px] shrink-0 items-center justify-center gap-1.5 rounded-md border-0 " +
  "px-3 text-[12px] font-medium transition-colors outline-none " +
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

/**
 * Comment-sized ember tint, for actions that sit inside a quoted thread.
 *
 * A second, smaller geometry rather than a shrunk {@link btnEmber}: on a
 * quote that is already indented, the full-size button outweighs the
 * comment it belongs to. Same tint, same borderlessness, same rule that
 * its box never changes between states.
 */
const BASE_XS =
  "inline-flex h-[22px] shrink-0 items-center justify-center gap-1 rounded border-0 " +
  "px-2 text-[11px] font-medium transition-colors outline-none " +
  "focus-visible:ring-[1.5px] focus-visible:ring-ring/60";

export const btnEmberXs = `${BASE_XS} bg-accent-ember/15 font-semibold text-accent-ember hover:bg-accent-ember/25`;

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
