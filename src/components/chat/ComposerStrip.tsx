import { Check, ChevronDown, CircleAlert, Clock } from "lucide-react";
import { useEffect, useId, useState } from "react";

import { AgentOrb } from "@/components/ui/agent-orb";
import type { OrbActivity } from "@/lib/orb-state";
import { cn } from "@/lib/utils";

import { TickingText } from "./TickingText";

/** Which occupant leads the collapsed strip. Lower wins. */
export const STRIP_PRIORITY = {
  error: 0,
  monitoring: 1,
  running: 2,
  finished: 3,
  queued: 4,
} as const;

export type StripOccupantKind = keyof typeof STRIP_PRIORITY;

/** The only coloured thing in a row. The surface itself stays neutral. */
export type StripMark =
  | { kind: "orb"; activity?: OrbActivity }
  | { kind: "monitoring" }
  | { kind: "finished" }
  | { kind: "queued" }
  | { kind: "error" };

export interface StripAction {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  title?: string;
}

/** One row. Slots are fixed so every occupant lines up on the same grid:
 *  20px mark | 12px semibold label | mono 11px detail (truncates) |
 *  optional mono 11px elapsed | one 26px action chip. */
export interface StripRow {
  id: string;
  mark: StripMark;
  label: string;
  detail?: string | null;
  elapsed?: (now: number) => string;
  action?: StripAction | null;
}

export interface StripOccupant {
  kind: StripOccupantKind;
  /** The single row shown while the strip is collapsed and this occupant
   *  leads it (e.g. "3 subagents running"). */
  summary: StripRow;
  /** One row per pending item, shown while the strip is open (running
   *  subagents flatten to one row each). */
  rows: StripRow[];
  /** Live work — draws the 1px ember sweep on the strip's top edge. */
  live?: boolean;
}

/** Rows are 34px; four fit before the list scrolls (4 × 34 + 3 × 3). */
const OPEN_MAX_HEIGHT = "max-h-[145px]";

/** Sweep geometry: a 1px, 30%-wide light travelling the top edge from
 *  -60% to 260% of the track. `cm-sweep` resolves `translateX` against the
 *  segment's own width, so the endpoints are track % ÷ 30%. Reduced motion
 *  is handled by the keyframe class, which drops the animation. */
const SWEEP_STYLE = {
  "--cm-sweep-from": "-200%",
  "--cm-sweep-to": "866%",
  animationDuration: "2.4s",
} as React.CSSProperties;

const STRIP_CHIP =
  "inline-flex h-[26px] shrink-0 items-center justify-center gap-1 rounded-[8px] bg-foreground/[0.05] px-2.5 text-[11px] font-semibold text-foreground/80 outline-none transition-colors hover:bg-foreground/[0.09] hover:text-foreground focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50";

/**
 * The one strip docked above the composer pill. It mirrors the scope
 * strip's chin under the draft composer, flipped: inset 20px on each side
 * (720px in the 760px column), a 1px border on every side except the one
 * facing the pill, 14px outer corners, and an 18px seam tucked under the
 * pill so the two read as layers rather than neighbours.
 *
 * Occupants compete for one slot. Collapsed, the strip is exactly one row:
 * the highest-priority occupant, plus a "+n" chip counting everything
 * else. Opening it lists every pending item in place — the transcript
 * gives way, nothing floats. Only the user opens or closes it (the chip or
 * Escape); arrivals and completions never touch that state, and the list
 * emptying is the one automatic close.
 */
export function ComposerStrip({
  occupants,
}: {
  occupants: ReadonlyArray<StripOccupant | null | undefined | false>;
}) {
  const present = occupants
    .filter((o): o is StripOccupant => !!o && o.rows.length > 0)
    .sort((a, b) => STRIP_PRIORITY[a.kind] - STRIP_PRIORITY[b.kind]);
  const rows = present.flatMap((occupant) =>
    occupant.rows.map((row) => ({ row, kind: occupant.kind })),
  );
  const total = rows.length;

  const listId = useId();
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (total === 0) setOpen(false);
  }, [total]);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (e: KeyboardEvent) => {
      // Anything that already consumed Escape (a composer popup closing)
      // keeps it.
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.preventDefault();
      setOpen(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open]);

  if (total === 0) return null;

  const lead = present[0];
  const rest = total - 1;
  const visible = open ? rows : [{ row: lead.summary, kind: lead.kind }];
  const sweep = open ? present.some((o) => o.live) : lead.live === true;

  const toggle =
    open || rest > 0 ? (
      <button
        type="button"
        data-testid="composer-strip-toggle"
        aria-expanded={open}
        aria-controls={listId}
        aria-label={open ? "Collapse pending activity" : `Show ${rest} more`}
        title={open ? "Collapse" : `Show ${rest} more`}
        onClick={() => setOpen((cur) => !cur)}
        className={cn(STRIP_CHIP, "min-w-[26px] px-1.5 font-mono")}
      >
        {open ? (
          <ChevronDown className="size-3" strokeWidth={2} aria-hidden />
        ) : (
          `+${rest}`
        )}
      </button>
    ) : null;

  return (
    <div
      data-testid="composer-strip"
      data-open={open || undefined}
      data-lead={lead.kind}
      className="relative z-0 w-full px-5"
    >
      <div className="relative -mb-[18px] overflow-hidden rounded-t-[14px] border border-b-0 border-border/70 bg-muted/20 pb-[18px]">
        {sweep && (
          <span
            data-testid="composer-strip-sweep"
            className="cm-sweep pointer-events-none absolute top-0 left-0 h-px w-[30%] bg-gradient-to-r from-transparent via-accent-ember to-transparent"
            style={SWEEP_STYLE}
            aria-hidden
          />
        )}
        <ul
          id={listId}
          aria-label="Pending activity"
          className={cn(
            "flex flex-col gap-[3px]",
            open && `${OPEN_MAX_HEIGHT} overflow-y-auto [scrollbar-width:thin]`,
          )}
        >
          {visible.map(({ row, kind }, index) => (
            <StripRowView
              key={row.id}
              row={row}
              kind={kind}
              trailing={index === 0 ? toggle : null}
            />
          ))}
        </ul>
      </div>
    </div>
  );
}

function StripRowView({
  row,
  kind,
  trailing,
}: {
  row: StripRow;
  kind: StripOccupantKind;
  trailing: React.ReactNode;
}) {
  return (
    <li
      data-testid="composer-strip-row"
      data-kind={kind}
      data-row-id={row.id}
      className="flex h-[34px] shrink-0 items-center gap-2.5 px-2"
    >
      <span className="flex size-5 shrink-0 items-center justify-center">
        <StripMarkView mark={row.mark} />
      </span>
      <span className="max-w-[40%] shrink-0 truncate whitespace-nowrap text-[12px] font-semibold text-foreground/80">
        {row.label}
      </span>
      <span
        className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted-foreground"
        title={row.detail ?? undefined}
      >
        {row.detail}
      </span>
      {row.elapsed && (
        <TickingText
          className="shrink-0 whitespace-nowrap font-mono text-[11px] text-muted-foreground"
          compute={row.elapsed}
        />
      )}
      {row.action && (
        <button
          type="button"
          data-testid="composer-strip-action"
          disabled={row.action.disabled}
          title={row.action.title}
          onClick={row.action.onClick}
          className={STRIP_CHIP}
        >
          {row.action.label}
        </button>
      )}
      {trailing}
    </li>
  );
}

function StripMarkView({ mark }: { mark: StripMark }) {
  switch (mark.kind) {
    case "orb":
      return <AgentOrb size={20} {...mark.activity} aria-hidden />;
    case "monitoring":
      // Calm background presence: a still dot, never a pulse.
      return (
        <span
          data-testid="composer-strip-monitoring-dot"
          className="size-2 rounded-full bg-warning"
          aria-hidden
        />
      );
    case "finished":
      return (
        <Check
          className="size-4 text-status-open"
          strokeWidth={1.8}
          aria-hidden
        />
      );
    case "queued":
      return (
        <Clock
          className="size-3.5 text-muted-foreground"
          strokeWidth={1.8}
          aria-hidden
        />
      );
    case "error":
      return (
        <CircleAlert
          className="size-4 text-danger"
          strokeWidth={1.8}
          aria-hidden
        />
      );
  }
}
