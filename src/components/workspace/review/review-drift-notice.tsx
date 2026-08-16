import type { ReactNode } from "react";
import { cn } from "@/lib/utils";
import { btnCard, btnCardStrong, btnEmberSolid, tzBody } from "./review-ui";

/**
 * Severity order. Exactly one notice shows at a time, and it is always
 * the most severe one that applies — a merged PR does not also need to
 * tell you the remote moved.
 */
export const DRIFT_PRIORITY = [
  "merged",
  "closed",
  "force-pushed",
  "conflict",
  "remote-ahead-dirty",
  "remote-ahead-clean",
  "submit-failed",
  "stale-data",
] as const;

export type DriftKind = (typeof DRIFT_PRIORITY)[number];

export type DriftTone = "violet" | "warn" | "sky" | "fail" | "muted";

export interface DriftAction {
  label: string;
  onClick: () => void;
  /** `strong` is emphasized-neutral; `primary` is ember-solid (Retry). */
  emphasis?: "card" | "strong" | "primary";
}

export interface DriftNotice {
  kind: DriftKind;
  tone: DriftTone;
  message: ReactNode;
  actions: DriftAction[];
}

const TONE_DOT: Record<DriftTone, string> = {
  violet: "bg-accent-violet",
  warn: "bg-status-working",
  // `status-remote` is the sky token, and it is the right one here for
  // more than its hue: a remote that moved under you is nearly always
  // another device or an agent pushing.
  sky: "bg-status-remote",
  fail: "bg-destructive",
  muted: "bg-muted-foreground",
};

/** Pick the most severe notice from everything that currently applies. */
export function mostSevere(notices: DriftNotice[]): DriftNotice | null {
  for (const kind of DRIFT_PRIORITY) {
    const found = notices.find((n) => n.kind === kind);
    if (found) return found;
  }
  return null;
}

/**
 * The single notice slot, directly above the action bar.
 *
 * It never covers content, never takes focus, and never rearranges what
 * is below it — it occupies its own row and pushes nothing around,
 * because the whole point is that you can keep reading while the world
 * changes underneath you.
 */
export function ReviewDriftNotice({ notice }: { notice: DriftNotice }) {
  return (
    <div
      role="status"
      data-testid="drift-notice"
      data-drift-kind={notice.kind}
      // Wraps rather than compresses. At panel width three actions can
      // take the whole row, and a message squeezed into the leftover
      // 20px is the one thing this notice may never do — the reason a
      // control is blocked has to stay readable in words (binding rule
      // 5). So the buttons drop to a second line instead.
      className="flex flex-wrap items-center gap-2 border-t border-border/40 bg-muted/30 px-3 py-2"
    >
      <span
        aria-hidden
        className={cn("size-2 shrink-0 rounded-full", TONE_DOT[notice.tone])}
      />
      <span className={cn("min-w-[11rem] flex-1 leading-snug text-foreground/80", tzBody)}>
        {notice.message}
      </span>
      {notice.actions.map((action) => (
        <button
          key={action.label}
          type="button"
          onClick={action.onClick}
          className={
            action.emphasis === "primary"
              ? btnEmberSolid
              : action.emphasis === "strong"
                ? btnCardStrong
                : btnCard
          }
        >
          {action.label}
        </button>
      ))}
    </div>
  );
}
