/**
 * The Subagents pane is a live-first watch surface: what is running RIGHT
 * NOW, at the top, and almost nothing else.
 *
 * The pane used to be an archive — every wave the thread ever spawned,
 * folded into groups, headed by a lifetime "12 / 40" counter. That is the
 * wrong shape for a surface you keep open while agents work: the thing you
 * are watching kept sliding down the list, and the number at the top only
 * ever went up. So the default view is now just `model.running`, newest
 * first, and the whole record moved to the History sub-view behind the
 * status foot's `History ›`.
 *
 * Two rules keep the live list from being a surface that erases itself:
 *
 * - **Completions linger.** A row that finishes while you are watching does
 *   not vanish mid-glance. Its orb becomes a green check, the activity line
 *   becomes the first line of its report, the timer freezes, and it holds
 *   its place for 8s before collapsing out. Hovering it pauses the hold,
 *   because "wait, what did that one say?" is the exact moment the row must
 *   not disappear. Rows that were already settled when the pane mounted
 *   never linger — they were never live to the user.
 * - **Failures hold.** Anything that failed or was stopped is pulled ABOVE
 *   the live list as a card that never fades. A failure is the one outcome
 *   the user has to act on, so it waits for an explicit Dismiss instead of
 *   timing itself out.
 *
 * When nothing is running the pane shows the last wave as a receipt — the
 * titles, the result lines, the frozen durations, and when it settled — so
 * an idle pane still answers "what just happened?" rather than going blank.
 */
import { Ban, Check, X } from "lucide-react";
import {
  Fragment,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { TickingText } from "@/components/chat/TickingText";
import { AgentOrb } from "@/components/ui/agent-orb";
import { subagentOrbActivity } from "@/lib/agent-chat/orb-activity";
import {
  formatElapsed,
  formatSettledAgo,
  isRunning,
  matchesHistoryFilter,
  subagentActivityLine,
  subagentElapsedMs,
  subagentGroupRollup,
  subagentHistoryGroups,
  subagentPaneModel,
  subagentRowTitle,
  subagentWaveSettledAt,
  subagentWaveStatus,
  subagentWaveTitle,
  subagentWaves,
  type SubagentHistoryFilter,
  type SubagentWave,
} from "@/lib/agent-chat/subagents";
import type { ChatViewItem, SubagentView } from "@/lib/agent-chat/types";
import { useCoarseClock } from "@/lib/use-coarse-clock";
import { cn } from "@/lib/utils";
import { useUIStore } from "@/stores/ui-store";

/** How long a just-finished row keeps its place in the live list. */
export const COMPLETION_LINGER_MS = 8000;
/** Collapse animation budget. jsdom never fires `transitionend`, so the
 *  unmount is driven by this timer rather than by the event. */
const COLLAPSE_MS = 300;

type LingerPhase = "hold" | "collapsing";

export function SubagentsPane({
  threadId,
  messages,
}: {
  threadId: string | null;
  messages: ChatViewItem[];
}) {
  const requestEnterSubagent = useUIStore((s) => s.requestEnterSubagent);
  const dismissSubagentAttention = useUIStore(
    (s) => s.dismissSubagentAttention,
  );
  const dismissed = useUIStore((s) => s.dismissedSubagentAttention);
  const historyOpen = useUIStore((s) => s.subagentsHistoryOpen);
  const setHistoryOpen = useUIStore((s) => s.setSubagentsHistoryOpen);

  const waves = useMemo(() => subagentWaves(messages), [messages]);
  const model = useMemo(
    () => subagentPaneModel(waves, new Set(dismissed)),
    [waves, dismissed],
  );
  /** Every row, newest wave first and newest row within a wave first — the
   *  order the live list reads in, and the one a lingering row is slotted
   *  back into so it does not jump on its way out. */
  const newestFirst = useMemo(() => {
    const out: SubagentView[] = [];
    for (let i = waves.length - 1; i >= 0; i--) {
      const wave = waves[i];
      for (let j = wave.subagents.length - 1; j >= 0; j--) {
        out.push(wave.subagents[j]);
      }
    }
    return out;
  }, [waves]);

  const titles = useMemo(() => waveRowTitles(waves), [waves]);
  const titleOf = useCallback(
    (view: SubagentView) => titles.get(view.id) ?? subagentRowTitle(view),
    [titles],
  );

  const linger = useLingeringCompletions(model.running, newestFirst);

  // The History sub-view is per-thread: switching threads returns to the
  // live list rather than leaving you in the previous thread's archive.
  // Guarded on an actual change so mounting never clobbers a caller that
  // opened History first.
  const lastThread = useRef(threadId);
  useEffect(() => {
    if (lastThread.current === threadId) return;
    lastThread.current = threadId;
    setHistoryOpen(false);
  }, [threadId, setHistoryOpen]);

  const openThread = useCallback(
    (subagentId: string) => {
      if (threadId) requestEnterSubagent(threadId, subagentId);
    },
    [threadId, requestEnterSubagent],
  );
  const canOpen = threadId != null;

  // Idle only: the receipt's "settled 4m ago" needs a clock, but a coarse
  // one — nothing here changes faster than a minute.
  const coarseNow = useCoarseClock(model.running.length === 0);

  if (waves.length === 0) {
    return (
      <div className="flex h-full items-center justify-center px-6 text-center">
        <p className="text-label text-muted-foreground/70">
          No subagents in this thread yet.
        </p>
      </div>
    );
  }

  if (historyOpen) {
    return (
      <div
        data-testid="subagents-pane"
        className="h-full min-h-0 overflow-y-auto px-3 py-3.5"
      >
        <HistoryView
          waves={waves}
          titleOf={titleOf}
          onOpen={openThread}
          canOpen={canOpen}
          onBack={() => setHistoryOpen(false)}
        />
      </div>
    );
  }

  const liveRows = newestFirst.filter(
    (view) => isRunning(view) || linger.phases.has(view.id),
  );
  // Header wave: the newest wave with something live in it. Falls back past
  // `currentWave` (null once the last agent settles) so a lingering row
  // keeps its heading for the seconds it is still on screen.
  const liveWave =
    model.currentWave ??
    lastWaveWith(waves, (view) => liveRows.includes(view)) ??
    model.lastWave;

  return (
    <div
      data-testid="subagents-pane"
      className="flex h-full min-h-0 flex-col gap-3 overflow-y-auto px-3 py-3.5"
    >
      {model.attention.length > 0 && (
        <section aria-label="Needs attention" className="flex flex-col gap-1.5">
          <SectionHeader
            label={`NEEDS ATTENTION · ${model.attention.length}`}
          />
          {model.attention.map((view) => (
            <AttentionCard
              key={view.id}
              view={view}
              title={titleOf(view)}
              canOpen={canOpen}
              onOpen={() => openThread(view.id)}
              onDismiss={() => dismissSubagentAttention(view.id)}
            />
          ))}
        </section>
      )}

      {liveRows.length > 0 && liveWave ? (
        <section aria-label="Working" className="flex flex-col gap-1.5">
          <div className="flex items-baseline gap-2">
            {model.running.length > 0 && (
              <SectionHeader label={`WORKING · ${model.running.length}`} />
            )}
            <span
              data-testid="live-wave-title"
              title={subagentWaveTitle(liveWave)}
              className="min-w-0 flex-1 truncate text-body-sm font-semibold text-foreground"
            >
              {subagentWaveTitle(liveWave)}
            </span>
            <TickingText
              active={model.running.length > 0}
              className="shrink-0 font-mono text-caption tabular-nums text-muted-foreground"
              compute={(now) => {
                const ms = subagentGroupRollup(
                  liveWave.subagents,
                  now,
                ).elapsedMs;
                return ms == null ? "" : formatElapsed(ms);
              }}
            />
          </div>
          {liveRows.map((view) => (
            <SubagentRow
              key={view.id}
              view={view}
              title={titleOf(view)}
              testId="live-row"
              phase={linger.phases.get(view.id)}
              onMouseEnter={() => linger.pause(view.id)}
              onMouseLeave={() => linger.resume(view.id)}
              canOpen={canOpen}
              onOpen={() => openThread(view.id)}
            />
          ))}
        </section>
      ) : (
        model.lastWave && (
          <IdleReceipt
            wave={model.lastWave}
            now={coarseNow}
            titleOf={titleOf}
            canOpen={canOpen}
            onOpen={openThread}
          />
        )
      )}
    </div>
  );
}

// ── Completion linger ──

/**
 * Track rows that finish while the pane is watching, so the live list can
 * hold them for a beat instead of deleting them out from under the eye.
 *
 * Only a running → completed transition observed ACROSS renders enters the
 * set: a row that was already settled when the pane mounted was never live
 * here, and failures are not in scope at all (they hold indefinitely as
 * attention cards, which is a stronger promise than an 8s fade).
 */
function useLingeringCompletions(
  running: readonly SubagentView[],
  all: readonly SubagentView[],
): {
  phases: Map<string, LingerPhase>;
  pause: (id: string) => void;
  resume: (id: string) => void;
} {
  const [phases, setPhases] = useState<Map<string, LingerPhase>>(
    () => new Map(),
  );
  /** Running ids as of the previous run; null until the first one, which is
   *  what makes "already settled at mount" a non-event. */
  const previouslyRunning = useRef<Set<string> | null>(null);
  /** Ids that have ever entered the linger set, so a re-render can't
   *  restart a hold that already expired. */
  const admitted = useRef<Set<string>>(new Set());
  const timers = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());

  const clearTimer = useCallback((id: string) => {
    const timer = timers.current.get(id);
    if (timer != null) clearTimeout(timer);
    timers.current.delete(id);
  }, []);

  const startHold = useCallback(
    (id: string) => {
      clearTimer(id);
      timers.current.set(
        id,
        setTimeout(() => {
          timers.current.delete(id);
          setPhases((prev) => {
            if (prev.get(id) !== "hold") return prev;
            const next = new Map(prev);
            next.set(id, "collapsing");
            return next;
          });
          timers.current.set(
            id,
            setTimeout(() => {
              timers.current.delete(id);
              setPhases((prev) => {
                if (!prev.has(id)) return prev;
                const next = new Map(prev);
                next.delete(id);
                return next;
              });
            }, COLLAPSE_MS),
          );
        }, COMPLETION_LINGER_MS),
      );
    },
    [clearTimer],
  );

  useEffect(() => {
    const nowRunning = new Set(running.map((view) => view.id));
    const previous = previouslyRunning.current;
    previouslyRunning.current = nowRunning;
    if (!previous) return;
    const entering = all.filter(
      (view) =>
        view.status === "completed" &&
        previous.has(view.id) &&
        !nowRunning.has(view.id) &&
        !admitted.current.has(view.id),
    );
    if (entering.length === 0) return;
    for (const view of entering) admitted.current.add(view.id);
    setPhases((prev) => {
      const next = new Map(prev);
      for (const view of entering) next.set(view.id, "hold");
      return next;
    });
    for (const view of entering) startHold(view.id);
  }, [running, all, startHold]);

  useEffect(() => {
    const pending = timers.current;
    return () => {
      for (const timer of pending.values()) clearTimeout(timer);
      pending.clear();
    };
  }, []);

  const pause = useCallback(
    (id: string) => {
      if (phases.get(id) !== "hold") return;
      clearTimer(id);
    },
    [phases, clearTimer],
  );
  const resume = useCallback(
    (id: string) => {
      if (phases.get(id) !== "hold") return;
      startHold(id);
    },
    [phases, startHold],
  );

  return { phases, pause, resume };
}

// ── Rows ──

type GlyphKind = "running" | "completed" | "failed" | "stopped";

function glyphKind(view: SubagentView): GlyphKind {
  if (isRunning(view)) return "running";
  if (view.status === "failed") return "failed";
  if (view.status === "stopped" || view.status === "interrupted") {
    return "stopped";
  }
  return "completed";
}

function RowGlyph({ view }: { view: SubagentView }) {
  const kind = glyphKind(view);
  if (kind === "running") {
    return (
      <span
        data-row-glyph="running"
        className="flex size-5 shrink-0 items-center justify-center"
      >
        <AgentOrb size={20} {...subagentOrbActivity(view)} aria-hidden />
      </span>
    );
  }
  const Icon = kind === "failed" ? X : kind === "stopped" ? Ban : Check;
  return (
    <span
      data-row-glyph={kind}
      className="flex size-5 shrink-0 items-center justify-center"
    >
      <Icon
        className={cn(
          "size-3",
          kind === "failed"
            ? "text-status-attention"
            : kind === "stopped"
              ? "text-status-working/80"
              : "text-status-open",
        )}
        strokeWidth={2.4}
        aria-hidden
      />
    </span>
  );
}

function SubagentRow({
  view,
  title,
  testId,
  phase,
  onMouseEnter,
  onMouseLeave,
  canOpen,
  onOpen,
}: {
  view: SubagentView;
  title: string;
  testId: string;
  phase?: LingerPhase;
  onMouseEnter?: () => void;
  onMouseLeave?: () => void;
  canOpen: boolean;
  onOpen: () => void;
}) {
  const running = isRunning(view);
  const activity = subagentActivityLine(view);
  return (
    <div
      data-testid={testId}
      data-linger={phase}
      data-subagent-id={view.id}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      className={cn(
        "overflow-hidden transition-[max-height,opacity] duration-[250ms] ease-out",
        phase === "collapsing"
          ? "max-h-0 opacity-0"
          : "max-h-[120px] opacity-100",
      )}
    >
      <button
        type="button"
        disabled={!canOpen}
        onClick={onOpen}
        aria-label={`Open ${title} thread`}
        className="flex w-full flex-col gap-0.5 rounded-lg bg-foreground/[0.03] px-2.5 py-2 text-left hover:bg-foreground/[0.05] disabled:cursor-default"
      >
        <span className="flex w-full items-center gap-2">
          <RowGlyph view={view} />
          <span className="min-w-0 flex-1 truncate text-body-sm font-semibold text-foreground">
            {title}
          </span>
          <SubagentModelBadge model={view.model} />
          <TickingText
            active={running}
            className="shrink-0 font-mono text-caption tabular-nums text-muted-foreground"
            compute={(now) => elapsedLabel(view, now)}
          />
        </span>
        <span
          data-subagent-excerpt
          title={activity}
          className="w-full truncate pl-7 font-mono text-caption leading-[1.45] text-muted-foreground"
        >
          {activity}
        </span>
      </button>
    </div>
  );
}

/**
 * A settled row the user has to deal with. Never fades: it sits above the
 * live list until "Dismiss", because a failure that times itself out is a
 * failure nobody saw.
 */
function AttentionCard({
  view,
  title,
  canOpen,
  onOpen,
  onDismiss,
}: {
  view: SubagentView;
  title: string;
  canOpen: boolean;
  onOpen: () => void;
  onDismiss: () => void;
}) {
  const activity = subagentActivityLine(view);
  return (
    <div
      data-testid="attention-card"
      data-subagent-id={view.id}
      className="rounded-lg border border-status-attention/25 bg-status-attention/5 px-2.5 py-[7px]"
    >
      <div className="flex w-full items-center gap-2">
        <RowGlyph view={view} />
        <span className="min-w-0 flex-1 truncate text-body-sm font-semibold text-foreground">
          {title}
        </span>
        <SubagentModelBadge model={view.model} />
        <TickingText
          active={false}
          className="shrink-0 font-mono text-caption tabular-nums text-muted-foreground"
          compute={(now) => elapsedLabel(view, now)}
        />
      </div>
      <p
        title={activity}
        className="mt-0.5 truncate pl-7 font-mono text-caption leading-[1.45] text-status-attention/85"
      >
        {activity}
      </p>
      <div className="mt-1 flex items-center gap-1 pl-7">
        <CardAction onClick={onOpen} disabled={!canOpen}>
          Open thread
        </CardAction>
        <CardAction onClick={onDismiss}>Dismiss</CardAction>
      </div>
    </div>
  );
}

function CardAction({
  onClick,
  disabled,
  children,
}: {
  onClick: () => void;
  disabled?: boolean;
  children: string;
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onClick}
      className="rounded-sm px-1.5 py-px font-mono text-caption text-muted-foreground hover:bg-foreground/[0.06] hover:text-foreground disabled:cursor-default disabled:opacity-50"
    >
      {children}
    </button>
  );
}

/** The last wave, read back as a receipt once nothing is running. */
function IdleReceipt({
  wave,
  now,
  titleOf,
  canOpen,
  onOpen,
}: {
  wave: SubagentWave;
  now: number;
  titleOf: (view: SubagentView) => string;
  canOpen: boolean;
  onOpen: (id: string) => void;
}) {
  const settledAt = subagentWaveSettledAt(wave);
  return (
    <section aria-label="Last run" className="flex flex-col gap-1.5">
      <div className="flex items-baseline gap-2">
        <span
          data-testid="receipt-title"
          title={subagentWaveTitle(wave)}
          className="min-w-0 flex-1 truncate text-body-sm font-semibold text-foreground"
        >
          {subagentWaveTitle(wave)}
        </span>
        {settledAt != null && (
          <span className="shrink-0 font-mono text-caption text-muted-foreground">
            {formatSettledAgo(now - settledAt)}
          </span>
        )}
      </div>
      {wave.subagents.map((view) => (
        <SubagentRow
          key={view.id}
          view={view}
          title={titleOf(view)}
          testId="receipt-row"
          canOpen={canOpen}
          onOpen={() => onOpen(view.id)}
        />
      ))}
    </section>
  );
}

// ── History ──

const HISTORY_FILTERS: { id: SubagentHistoryFilter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "done", label: "Done" },
  { id: "failed", label: "Failed" },
  { id: "stopped", label: "Stopped" },
];

function HistoryView({
  waves,
  titleOf,
  onOpen,
  canOpen,
  onBack,
}: {
  waves: SubagentWave[];
  titleOf: (view: SubagentView) => string;
  onOpen: (id: string) => void;
  canOpen: boolean;
  onBack: () => void;
}) {
  const [filter, setFilter] = useState<SubagentHistoryFilter>("all");
  const groups = useMemo(() => subagentHistoryGroups(waves), [waves]);

  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex items-center gap-1 font-mono text-caption text-muted-foreground">
        <button
          type="button"
          onClick={onBack}
          aria-label="Back to Subagents"
          className="rounded-sm px-1 py-px hover:bg-foreground/[0.06] hover:text-foreground"
        >
          ‹ Subagents
        </button>
        <span aria-hidden className="text-muted-foreground/50">
          /
        </span>
        <span className="text-foreground/80">History</span>
      </div>

      <div role="group" aria-label="History filter" className="flex gap-1">
        {HISTORY_FILTERS.map(({ id, label }) => (
          <button
            key={id}
            type="button"
            aria-pressed={filter === id}
            onClick={() => setFilter(id)}
            className={cn(
              "rounded-sm px-1.5 py-px font-mono text-caption",
              filter === id
                ? "bg-foreground/[0.08] text-foreground"
                : "text-muted-foreground hover:bg-foreground/[0.04]",
            )}
          >
            {label}
          </button>
        ))}
      </div>

      <div className="flex flex-col gap-1.5">
        {groups.map((group) => {
          const kept = group.waves
            .map((wave) => ({
              wave,
              rows: wave.subagents.filter((view) =>
                matchesHistoryFilter(view, filter),
              ),
            }))
            .filter((entry) => entry.rows.length > 0);
          if (kept.length === 0) return null;
          return (
            <Fragment key={group.waves[0].id}>
              {group.prompt != null && <PromptDivider text={group.prompt} />}
              {kept.map(({ wave, rows }) => (
                <section
                  key={wave.id}
                  aria-label={subagentWaveTitle(wave)}
                  data-wave-status={subagentWaveStatus(wave.subagents)}
                  className="flex flex-col gap-1 rounded-lg bg-foreground/[0.03] px-1.5 py-1.5"
                >
                  <span className="truncate px-1 text-body-sm font-semibold text-foreground">
                    {subagentWaveTitle(wave)}
                  </span>
                  {rows.map((view) => (
                    <SubagentRow
                      key={view.id}
                      view={view}
                      title={titleOf(view)}
                      testId="history-row"
                      canOpen={canOpen}
                      onOpen={() => onOpen(view.id)}
                    />
                  ))}
                </section>
              ))}
            </Fragment>
          );
        })}
      </div>
    </div>
  );
}

/** The user prompt that started a turn, shown once above its waves. */
function PromptDivider({ text }: { text: string }) {
  return (
    <p
      data-testid="wave-prompt"
      title={text}
      className="mt-2 truncate px-1 text-caption text-muted-foreground/80 first:mt-0"
    >
      <span className="mr-1 text-muted-foreground/50" aria-hidden>
        ›
      </span>
      {text}
    </p>
  );
}

// ── Shared bits ──

function SectionHeader({ label }: { label: string }) {
  return (
    <span className="shrink-0 font-mono text-caption tracking-[0.14em] text-muted-foreground/70 uppercase">
      {label}
    </span>
  );
}

/**
 * Provider-neutral model identity. Snapshot model ids are intentionally
 * opaque cross-provider strings, so the pane presents the value exactly as
 * reported instead of guessing at a vendor-specific display name. Long ids
 * truncate in the row; the native title keeps the full value one hover away.
 */
function SubagentModelBadge({ model }: { model?: string }) {
  const value = model?.trim();
  if (!value) return null;

  return (
    <span
      data-subagent-model={value}
      title={`Model: ${value}`}
      className="inline-flex h-[17px] max-w-[96px] min-w-0 shrink-0 items-center gap-1 overflow-hidden rounded-sm border border-foreground/[0.08] bg-background/55 px-1.5 font-mono text-micro text-muted-foreground shadow-[inset_0_1px_0_color-mix(in_oklch,var(--foreground)_2.5%,transparent)]"
    >
      <span
        className="size-1 shrink-0 rounded-full bg-accent-ember/75"
        aria-hidden
      />
      <span className="truncate">{value}</span>
    </span>
  );
}

function elapsedLabel(view: SubagentView, now: number): string {
  const ms = subagentElapsedMs(view, now);
  return ms == null ? "" : formatElapsed(ms);
}

/** Row titles per wave, with an ordinal suffix only where a wave repeats
 *  the same title ("Explore 1" / "Explore 2"). */
function waveRowTitles(waves: readonly SubagentWave[]): Map<string, string> {
  const out = new Map<string, string>();
  for (const wave of waves) {
    const counts = new Map<string, number>();
    for (const view of wave.subagents) {
      const title = subagentRowTitle(view);
      counts.set(title, (counts.get(title) ?? 0) + 1);
    }
    const seen = new Map<string, number>();
    for (const view of wave.subagents) {
      const title = subagentRowTitle(view);
      if ((counts.get(title) ?? 0) < 2) {
        out.set(view.id, title);
        continue;
      }
      const n = (seen.get(title) ?? 0) + 1;
      seen.set(title, n);
      out.set(view.id, `${title} ${n}`);
    }
  }
  return out;
}

/** Newest wave holding a row the predicate accepts. */
function lastWaveWith(
  waves: readonly SubagentWave[],
  match: (view: SubagentView) => boolean,
): SubagentWave | null {
  for (let i = waves.length - 1; i >= 0; i--) {
    if (waves[i].subagents.some(match)) return waves[i];
  }
  return null;
}
