import {
  Check,
  ChevronDown,
  ChevronRight,
  ChevronUp,
  CircleAlert,
  Clock,
  EllipsisVertical,
} from "lucide-react";
import { useEffect, useId, useState } from "react";

import { AgentOrb } from "@/components/ui/agent-orb";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  formatClockTime,
  formatGoalAge,
  type ThreadGoal,
} from "@/lib/agent-chat/goal";
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

/**
 * The thread's standing `/goal`. Every action is text: Resume and Clear
 * send a phrase through the normal send path, Copy and Jump never send.
 */
export interface StripGoal {
  goal: ThreadGoal;
  /** The literal text Resume sends, shown in the `sends` box. */
  resumePhrase: string;
  onResume: () => void;
  /** Put `resumePhrase` in the composer instead of sending it. */
  onEditResume: () => void;
  onClear: () => void;
  onCopy: () => void;
  /** Scroll to the turn that set the goal. `null` once that turn has left
   *  the loaded transcript. */
  onJump?: (() => void) | null;
  /** Interrupted only: where the run stopped, read off the transcript. */
  stopped?: {
    at: number | null;
    /** Opening words of the last reply, for `stopped after "…"`. */
    after: string | null;
    onJump: (() => void) | null;
  } | null;
}

/** One strip row's resting height. Exported so the occupant row, the goal
 *  row and the tests all read the same value from one place instead of
 *  three copies of a pixel literal. */
export const STRIP_ROW_HEIGHT = "h-[34px]";

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
  "inline-flex h-[26px] shrink-0 items-center justify-center gap-1 rounded-md bg-foreground/[0.05] px-2.5 text-label font-semibold text-foreground/80 outline-none transition-colors hover:bg-foreground/[0.09] hover:text-foreground focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50";

// Goal row controls, drawn to Canvas-12 2a / 4a: 26px, 6px corners, 11px/600.
const GOAL_FOCUS = "outline-none focus-visible:ring-1 focus-visible:ring-ring";
/** Quiet text action (Copy, Clear beside Resume). */
const GOAL_CHIP = cn(
  "inline-flex h-[26px] shrink-0 items-center justify-center gap-[5px] rounded-sm px-2 text-label font-semibold text-muted-foreground transition-colors hover:bg-foreground/[0.07] hover:text-foreground",
  GOAL_FOCUS,
);
/** Clear when it is the strongest action in the row. */
const GOAL_CHIP_OUTLINE = cn(
  GOAL_CHIP,
  "border border-foreground/[0.16] text-foreground/85",
);
/** Hide and the `+n` pill: filled, with a trailing chevron. */
const GOAL_CHIP_FILLED = cn(
  GOAL_CHIP,
  "bg-foreground/[0.07] px-[7px] text-foreground/80 hover:bg-foreground/[0.11]",
);
/** The resting chevron and the overflow trigger: icon only, no fill. */
const GOAL_ICON_BUTTON = cn(
  "inline-flex size-[26px] shrink-0 items-center justify-center rounded-sm text-muted-foreground transition-colors hover:bg-foreground/[0.07] hover:text-foreground",
  GOAL_FOCUS,
);
/** Resume is the one solid control in the strip: an interrupted goal is the
 *  only occupant that asks for a decision. */
const GOAL_RESUME = cn(
  "inline-flex h-[26px] shrink-0 items-center justify-center rounded-sm bg-status-working px-2.5 text-label font-bold text-status-working-foreground transition-[filter] hover:brightness-110",
  GOAL_FOCUS,
);
const GOAL_META =
  "shrink-0 whitespace-nowrap font-mono text-label text-muted-foreground";
const GOAL_LINK = cn(
  "inline-flex shrink-0 items-center gap-[5px] rounded-sm transition-colors hover:text-foreground",
  GOAL_FOCUS,
);
const GOAL_MENU_ITEM = "h-[26px] rounded-sm px-2 py-0 text-body-sm";

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
 *
 * A goal outranks every occupant: it always holds the visible row and the
 * rest count into its `+n`. Opening a goal-led strip drills into the goal
 * rather than listing occupants; the drill-in names what else is pending,
 * and that line opens the occupant list beneath it.
 */
export function ComposerStrip({
  occupants,
  goal = null,
}: {
  occupants: ReadonlyArray<StripOccupant | null | undefined | false>;
  goal?: StripGoal | null;
}) {
  const present = occupants
    .filter((o): o is StripOccupant => !!o && o.rows.length > 0)
    .sort((a, b) => STRIP_PRIORITY[a.kind] - STRIP_PRIORITY[b.kind]);
  const rows = present.flatMap((occupant) =>
    occupant.rows.map((row) => ({ row, kind: occupant.kind })),
  );
  const total = rows.length;
  const hasGoal = goal !== null;

  const listId = useId();
  const [open, setOpen] = useState(false);
  const [goalOpen, setGoalOpen] = useState(false);

  useEffect(() => {
    if (total === 0) setOpen(false);
  }, [total]);

  useEffect(() => {
    if (!hasGoal) setGoalOpen(false);
  }, [hasGoal]);

  const anyOpen = open || goalOpen;
  useEffect(() => {
    if (!anyOpen) return;
    const onKeyDown = (e: KeyboardEvent) => {
      // Anything that already consumed Escape (a composer popup closing)
      // keeps it.
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.preventDefault();
      setOpen(false);
      setGoalOpen(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [anyOpen]);

  if (total === 0 && !goal) return null;

  const lead = present[0];
  const interrupted = goal?.goal.status === "interrupted";
  // An interrupted goal draws a still amber edge instead; the two never
  // share the top edge.
  const sweep = interrupted
    ? false
    : open || goal
      ? present.some((o) => o.live)
      : lead.live === true;

  const rest = total - 1;
  const toggle =
    !goal && (open || rest > 0) ? (
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
      data-lead={goal ? "goal" : lead.kind}
      className="relative z-0 w-full px-5"
    >
      <div className="relative -mb-[18px] overflow-hidden rounded-t-[14px] border border-b-0 border-border/70 bg-muted/20 pb-[18px]">
        {interrupted && (
          <>
            <span
              data-testid="composer-strip-goal-tint"
              className="pointer-events-none absolute inset-0 bg-[color-mix(in_oklch,var(--status-working)_6%,transparent)]"
              aria-hidden
            />
            <span
              data-testid="composer-strip-goal-edge"
              className="pointer-events-none absolute top-0 left-[16%] h-px w-[30%] bg-gradient-to-r from-transparent via-status-working to-transparent"
              aria-hidden
            />
          </>
        )}
        {sweep && (
          <span
            data-testid="composer-strip-sweep"
            className="cm-sweep pointer-events-none absolute top-0 left-0 h-px w-[30%] bg-gradient-to-r from-transparent via-accent-ember to-transparent"
            style={SWEEP_STYLE}
            aria-hidden
          />
        )}
        {goal ? (
          <ul aria-label="Pending activity" className="relative flex flex-col">
            <GoalRowView
              strip={goal}
              open={goalOpen}
              onToggle={() => {
                setGoalOpen((cur) => !cur);
                setOpen(false);
              }}
              others={total}
              summary={occupantSummary(present)}
              listOpen={open}
              listId={listId}
              onToggleList={() => setOpen((cur) => !cur)}
            />
            {goalOpen && open && (
              <li>
                <ul
                  id={listId}
                  aria-label="Other activity"
                  className={cn(
                    "flex flex-col gap-[3px] overflow-y-auto [scrollbar-width:thin]",
                    OPEN_MAX_HEIGHT,
                  )}
                >
                  {rows.map(({ row, kind }) => (
                    <StripRowView
                      key={row.id}
                      row={row}
                      kind={kind}
                      trailing={null}
                    />
                  ))}
                </ul>
              </li>
            )}
          </ul>
        ) : (
          <ul
            id={listId}
            aria-label="Pending activity"
            className={cn(
              "relative flex flex-col gap-[3px]",
              open &&
                `${OPEN_MAX_HEIGHT} overflow-y-auto [scrollbar-width:thin]`,
            )}
          >
            {(open ? rows : [{ row: lead.summary, kind: lead.kind }]).map(
              ({ row, kind }, index) => (
                <StripRowView
                  key={row.id}
                  row={row}
                  kind={kind}
                  trailing={index === 0 ? toggle : null}
                />
              ),
            )}
          </ul>
        )}
      </div>
    </div>
  );
}

/** "2 subagents running · 1 message queued", in strip priority order. */
function occupantSummary(present: StripOccupant[]): string | null {
  const parts = present.map((o) => {
    const n = o.rows.length;
    switch (o.kind) {
      case "running":
        return `${n} subagent${n === 1 ? "" : "s"} running`;
      case "queued":
        return `${n} message${n === 1 ? "" : "s"} queued`;
      case "finished":
        return "subagents finished";
      case "monitoring":
        return "monitoring";
      case "error":
        return "session error";
    }
  });
  return parts.length > 0 ? parts.join(" · ") : null;
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
      className={cn(
        "flex shrink-0 items-center gap-2.5 px-2",
        STRIP_ROW_HEIGHT,
      )}
    >
      <span className="flex size-5 shrink-0 items-center justify-center">
        <StripMarkView mark={row.mark} />
      </span>
      <span className="max-w-[40%] shrink-0 truncate whitespace-nowrap text-body-sm font-semibold text-foreground/80">
        {row.label}
      </span>
      <span
        className="min-w-0 flex-1 truncate font-mono text-label text-muted-foreground"
        title={row.detail ?? undefined}
      >
        {row.detail}
      </span>
      {row.elapsed && (
        <TickingText
          className="shrink-0 whitespace-nowrap font-mono text-label text-muted-foreground"
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

/**
 * The goal row. Collapsed it is one strip row, so a goal appearing never
 * changes the strip's height. Green marks that a goal is standing, never
 * how it is going: Codemux can't see the provider's loop, so the row shows
 * only what the user set and when.
 */
function GoalRowView({
  strip,
  open,
  onToggle,
  others,
  summary,
  listOpen,
  listId,
  onToggleList,
}: {
  strip: StripGoal;
  open: boolean;
  onToggle: () => void;
  /** Occupant rows counted into `+n`. */
  others: number;
  summary: string | null;
  listOpen: boolean;
  listId: string;
  onToggleList: () => void;
}) {
  const { goal } = strip;
  const interrupted = goal.status === "interrupted";
  const detailsId = useId();
  const setAt = goal.setAt;
  const stopped = interrupted ? (strip.stopped ?? null) : null;
  const stoppedAt = stopped?.at ?? null;

  const jump = interrupted && stopped?.onJump
    ? { label: "Jump to last activity", onClick: stopped.onJump }
    : strip.onJump
      ? { label: "Jump to message", onClick: strip.onJump }
      : null;
  const note = interrupted
    ? stopped?.after
      ? `stopped after "${stopped.after}"`
      : "stopped before the run finished"
    : null;

  return (
    <li
      data-testid="composer-strip-goal"
      data-kind="goal"
      data-status={goal.status}
      data-open={open || undefined}
      className="flex shrink-0 flex-col"
    >
      <div className={cn("flex items-center gap-2 px-2", STRIP_ROW_HEIGHT)}>
        <span className="flex size-5 shrink-0 items-center justify-center">
          {interrupted ? (
            <GoalPauseGlyph className="text-status-working" />
          ) : (
            <GoalTargetGlyph className="text-status-open" />
          )}
        </span>
        <button
          type="button"
          data-testid="composer-strip-goal-toggle"
          aria-expanded={open}
          aria-controls={open ? detailsId : undefined}
          onClick={onToggle}
          className={cn(
            "flex min-w-0 flex-1 items-center gap-2 self-stretch rounded-sm text-left",
            GOAL_FOCUS,
          )}
        >
          <span className="shrink-0 whitespace-nowrap text-body-sm font-semibold text-foreground/80">
            {interrupted ? "Goal interrupted" : "Goal"}
          </span>
          {!open ? (
            <span
              className="min-w-0 flex-1 truncate text-body-sm text-muted-foreground"
              title={goal.text}
            >
              {goal.text}
            </span>
          ) : interrupted ? (
            stoppedAt !== null && (
              <TickingText
                testId="composer-strip-goal-meta"
                className={GOAL_META}
                compute={(now) =>
                  `stopped ${formatClockTime(stoppedAt)} · idle ${formatGoalAge(now - stoppedAt)}`
                }
                intervalMs={15_000}
              />
            )
          ) : (
            <TickingText
              testId="composer-strip-goal-meta"
              className={GOAL_META}
              compute={(now) =>
                `set ${formatClockTime(setAt)} · ${formatGoalAge(now - setAt)} ago`
              }
              intervalMs={15_000}
            />
          )}
        </button>
        {open ? (
          <>
            {interrupted ? (
              <button
                type="button"
                data-testid="composer-strip-goal-resume"
                onClick={strip.onResume}
                className={GOAL_RESUME}
              >
                Resume
              </button>
            ) : (
              <button type="button" onClick={strip.onCopy} className={GOAL_CHIP}>
                Copy
              </button>
            )}
            <button
              type="button"
              onClick={strip.onClear}
              className={interrupted ? GOAL_CHIP : GOAL_CHIP_OUTLINE}
            >
              Clear
            </button>
            <button
              type="button"
              onClick={onToggle}
              className={GOAL_CHIP_FILLED}
            >
              Hide
              <ChevronDown
                className="size-[9px] opacity-60"
                strokeWidth={2.2}
                aria-hidden
              />
            </button>
          </>
        ) : (
          <>
            {interrupted ? (
              <>
                <button
                  type="button"
                  data-testid="composer-strip-goal-resume"
                  title={`Sends “${strip.resumePhrase}”`}
                  onClick={strip.onResume}
                  className={GOAL_RESUME}
                >
                  Resume
                </button>
                <GoalOverflowMenu strip={strip} />
              </>
            ) : (
              <TickingText
                testId="composer-strip-goal-age"
                className={GOAL_META}
                compute={(now) => formatGoalAge(now - setAt)}
                intervalMs={15_000}
              />
            )}
            {others > 0 ? (
              <button
                type="button"
                data-testid="composer-strip-goal-more"
                aria-expanded={false}
                aria-label={`Show goal and ${others} more`}
                title={`Show goal and ${others} more`}
                onClick={onToggle}
                className={GOAL_CHIP_FILLED}
              >
                <span className="font-mono">+{others}</span>
                <ChevronUp
                  className="size-[9px] opacity-60"
                  strokeWidth={2.2}
                  aria-hidden
                />
              </button>
            ) : (
              !interrupted && (
                <button
                  type="button"
                  aria-label="Show goal"
                  title="Show goal"
                  onClick={onToggle}
                  className={GOAL_ICON_BUTTON}
                >
                  <ChevronUp className="size-2.5" strokeWidth={2} aria-hidden />
                </button>
              )
            )}
          </>
        )}
      </div>
      {open && (
        <div
          id={detailsId}
          data-testid="composer-strip-goal-details"
          className="flex flex-col gap-[7px] pt-px pr-2.5 pb-[3px] pl-9"
        >
          <p className="max-h-[102px] overflow-y-auto whitespace-pre-wrap break-words text-body-sm leading-[1.55] text-foreground/[0.82] [scrollbar-width:thin]">
            {goal.text}
          </p>
          {interrupted && (
            <div
              data-testid="composer-strip-goal-sends"
              className="flex min-w-0 items-center gap-2 rounded-md border border-border/70 bg-foreground/[0.02] px-[9px] py-1.5"
            >
              <span className="shrink-0 font-mono text-caption font-semibold tracking-[0.08em] text-muted-foreground uppercase">
                sends
              </span>
              <code
                className="min-w-0 flex-1 truncate font-mono text-label text-accent-ember"
                title={strip.resumePhrase}
              >
                {strip.resumePhrase}
              </code>
              <button
                type="button"
                onClick={strip.onEditResume}
                className={cn(
                  "shrink-0 rounded-sm font-mono text-label text-muted-foreground transition-colors hover:text-foreground",
                  GOAL_FOCUS,
                )}
              >
                edit before sending
              </button>
            </div>
          )}
          {(jump || note || summary) && (
            <div className="flex min-w-0 items-center gap-2.5 font-mono text-label text-muted-foreground">
              {jump && (
                <button type="button" onClick={jump.onClick} className={GOAL_LINK}>
                  {jump.label}
                  <ChevronRight
                    className="size-[9px]"
                    strokeWidth={2.2}
                    aria-hidden
                  />
                </button>
              )}
              {[note, summary]
                .filter((part): part is string => !!part)
                .map((part, index) => (
                  <span
                    key={part}
                    className="flex min-w-0 items-center gap-2.5"
                  >
                    {(jump || index > 0) && (
                      <span aria-hidden className="opacity-40">
                        ·
                      </span>
                    )}
                    {part === summary ? (
                      <button
                        type="button"
                        data-testid="composer-strip-goal-others"
                        aria-expanded={listOpen}
                        aria-controls={listOpen ? listId : undefined}
                        onClick={onToggleList}
                        className={cn(
                          "min-w-0 truncate rounded-sm text-left transition-colors hover:text-foreground",
                          GOAL_FOCUS,
                        )}
                      >
                        {part}
                      </button>
                    ) : (
                      <span className="min-w-0 truncate">{part}</span>
                    )}
                  </span>
                ))}
            </div>
          )}
        </div>
      )}
    </li>
  );
}

function GoalOverflowMenu({ strip }: { strip: StripGoal }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          data-testid="composer-strip-goal-menu"
          aria-label="Goal actions"
          title="Goal actions"
          className={GOAL_ICON_BUTTON}
        >
          <EllipsisVertical className="size-3" strokeWidth={2} aria-hidden />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" side="top" className="w-[168px] p-1">
        <DropdownMenuItem className={GOAL_MENU_ITEM} onSelect={strip.onCopy}>
          Copy goal text
        </DropdownMenuItem>
        <DropdownMenuItem
          className={GOAL_MENU_ITEM}
          disabled={!strip.onJump}
          onSelect={() => strip.onJump?.()}
        >
          Jump to message
        </DropdownMenuItem>
        <DropdownMenuSeparator className="mx-1.5 my-[3px]" />
        <DropdownMenuItem className={GOAL_MENU_ITEM} onSelect={strip.onClear}>
          Clear goal
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** Canvas-12 target: a ring with a filled centre. */
function GoalTargetGlyph({ className }: { className?: string }) {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      className={className}
      aria-hidden
    >
      <circle cx="8" cy="8" r="6" />
      <circle cx="8" cy="8" r="2.4" fill="currentColor" stroke="none" />
    </svg>
  );
}

/** Canvas-12 pause: the same ring with two bars. */
function GoalPauseGlyph({ className }: { className?: string }) {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      className={className}
      aria-hidden
    >
      <circle cx="8" cy="8" r="6" />
      <path d="M6.4 5.8v4.4M9.6 5.8v4.4" strokeLinecap="round" />
    </svg>
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
