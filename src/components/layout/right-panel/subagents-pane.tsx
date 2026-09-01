/**
 * The Subagents pane is Turn 2's watch surface. The transcript keeps only a
 * 32px work-log row; live detail, progress, and thread drill-in live here.
 *
 * Rows group by spawn wave (one `subagent_run` card each). A wave folds to
 * a single header — what the agents were asked to do, rollup, longest
 * duration — so tens of finished agents become a few groups. The user
 * prompt that started a turn appears once as a divider above that turn's
 * waves (an orchestrating turn can spawn a dozen). The latest wave and any
 * wave still running stay open; older ones fold. Each child row is two
 * lines: name / model / duration, then the first line of the agent's
 * report, so a finished row reads as a receipt rather than a tombstone.
 */
import { Ban, Check, ChevronRight, X } from "lucide-react";
import { Fragment, useMemo, useState } from "react";

import { TickingText } from "@/components/chat/TickingText";
import { formatCompactTokens } from "@/components/chat/WorkflowRunCard";
import { AgentOrb } from "@/components/ui/agent-orb";
import { subagentOrbActivity } from "@/lib/agent-chat/orb-activity";
import {
  formatElapsed,
  isRunning,
  statusTone,
  subagentActivityLine,
  subagentElapsedMs,
  subagentGroupRollup,
  subagentOrdinals,
  subagentToolCount,
  subagentWaveStatus,
  subagentWaveTitle,
  subagentWaves,
  type SubagentWave,
} from "@/lib/agent-chat/subagents";
import type {
  ChatViewItem,
  SubagentView,
  SubagentViewStatus,
} from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";
import { useUIStore } from "@/stores/ui-store";

export function SubagentsPane({
  threadId,
  messages,
}: {
  threadId: string | null;
  messages: ChatViewItem[];
}) {
  const requestEnterSubagent = useUIStore((s) => s.requestEnterSubagent);
  const waves = useMemo(() => subagentWaves(messages), [messages]);
  const subagents = useMemo(
    () => waves.flatMap((wave) => wave.subagents),
    [waves],
  );
  // Explicit user folds/unfolds, keyed by wave id. Anything not in here
  // follows the default: open while running or when it is the latest wave.
  const [folds, setFolds] = useState<Record<string, boolean>>({});

  const active = subagents.filter(isRunning);
  const finished = subagents.length - active.length;
  const progress =
    subagents.length === 0
      ? 0
      : Math.round((finished / subagents.length) * 100);

  if (subagents.length === 0) {
    return (
      <div className="flex h-full items-center justify-center px-6 text-center">
        <p className="text-[11px] text-muted-foreground/70">
          No subagents in this thread yet.
        </p>
      </div>
    );
  }

  const openThread = (subagentId: string) => {
    if (threadId) requestEnterSubagent(threadId, subagentId);
  };
  const latestWaveId = waves[waves.length - 1]?.id;

  return (
    <div
      data-testid="subagents-pane"
      className="h-full min-h-0 overflow-y-auto px-3 py-3.5"
    >
      <div className="flex h-5 items-center gap-2.5">
        <span className="shrink-0 font-mono text-[10px] text-foreground/80">
          {finished} / {subagents.length}
        </span>
        <span className="h-[3px] min-w-0 flex-1 overflow-hidden rounded-full bg-foreground/[0.07]">
          <span
            className="block h-full rounded-full bg-status-open transition-[width] duration-300"
            style={{ width: `${progress}%` }}
          />
        </span>
        <TickingText
          active={active.length > 0}
          className="ml-auto font-mono text-[10px] tabular-nums text-muted-foreground"
          compute={(now) => elapsedLabel(subagents, now)}
        />
      </div>
      <div
        className="sr-only"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={subagents.length}
        aria-valuenow={finished}
        aria-label="Subagent progress"
      />

      <div className="mt-3 flex flex-col gap-1.5">
        {waves.map((wave, i) => {
          const running = wave.subagents.some(isRunning);
          const open = folds[wave.id] ?? (running || wave.id === latestWaveId);
          const newTurn =
            wave.prompt != null && wave.promptId !== waves[i - 1]?.promptId;
          return (
            <Fragment key={wave.id}>
              {newTurn && <PromptDivider text={wave.prompt ?? ""} />}
              <WaveGroup
                wave={wave}
                open={open}
                onToggle={() =>
                  setFolds((prev) => ({ ...prev, [wave.id]: !open }))
                }
                onOpen={openThread}
                canOpen={threadId != null}
              />
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
      className="mt-2 truncate px-1 text-[10px] text-muted-foreground/80 first:mt-0"
    >
      <span className="mr-1 text-muted-foreground/50" aria-hidden>
        ›
      </span>
      {text}
    </p>
  );
}

function WaveGroup({
  wave,
  open,
  onToggle,
  onOpen,
  canOpen,
}: {
  wave: SubagentWave;
  open: boolean;
  onToggle: () => void;
  onOpen: (subagentId: string) => void;
  canOpen: boolean;
}) {
  const status = subagentWaveStatus(wave.subagents);
  const title = subagentWaveTitle(wave);
  const ordinals = subagentOrdinals(wave.subagents);
  // Not `status === "running"`: that rollup ranks a failure above a live
  // agent, which would freeze the timer while a sibling is still going.
  const running = wave.subagents.some(isRunning);
  return (
    <section
      data-wave-status={status}
      aria-label={title}
      className="overflow-hidden rounded-[10px] bg-foreground/[0.03]"
    >
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        className="flex h-[38px] w-full items-center gap-2 px-2.5 text-left hover:bg-foreground/[0.03]"
      >
        <WaveGlyph status={status} />
        <span className="flex min-w-0 flex-1 flex-col gap-px">
          <span className="truncate text-[11.5px] font-semibold text-foreground">
            {title}
          </span>
          <span className="truncate font-mono text-[9px] text-muted-foreground">
            {waveMeta(wave.subagents)}
          </span>
        </span>
        <TickingText
          active={running}
          className="shrink-0 font-mono text-[9.5px] tabular-nums text-muted-foreground"
          compute={(now) => elapsedLabel(wave.subagents, now)}
        />
        <ChevronRight
          className={cn(
            "size-[11px] shrink-0 text-muted-foreground transition-transform duration-150",
            open && "rotate-90",
          )}
          aria-hidden
        />
      </button>
      {open && (
        <div className="border-t border-border/60 px-1 pt-1 pb-1">
          {wave.subagents.map((subagent) => (
            <WaveRow
              key={subagent.id}
              subagent={subagent}
              ordinal={ordinals.get(subagent.id) ?? null}
              onOpen={() => onOpen(subagent.id)}
              canOpen={canOpen}
            />
          ))}
        </div>
      )}
    </section>
  );
}

function WaveRow({
  subagent,
  ordinal,
  onOpen,
  canOpen,
}: {
  subagent: SubagentView;
  ordinal: number | null;
  onOpen: () => void;
  canOpen: boolean;
}) {
  const name = subagent.name ?? subagent.agentType ?? "Subagent";
  const label = ordinal == null ? name : `${name} ${ordinal}`;
  const running = isRunning(subagent);
  const excerpt = subagentActivityLine(subagent);
  return (
    <button
      type="button"
      disabled={!canOpen}
      onClick={onOpen}
      aria-label={`Open ${label} thread`}
      className="group flex w-full flex-col gap-0.5 rounded-[7px] px-1.5 pt-1.5 pb-[7px] text-left hover:bg-foreground/[0.04] disabled:cursor-default"
    >
      <span className="grid w-full grid-cols-[20px_minmax(0,1fr)_auto_auto_10px] items-center gap-1.5">
        <RowGlyph subagent={subagent} />
        <span className="min-w-0 truncate font-mono text-[10.5px] text-foreground/80">
          {name}
          {ordinal != null && (
            <span className="text-muted-foreground/70"> {ordinal}</span>
          )}
        </span>
        <SubagentModelBadge model={subagent.model} />
        <TickingText
          active={running}
          className="min-w-[38px] shrink-0 text-right font-mono text-[9.5px] tabular-nums text-muted-foreground"
          compute={(now) => rowTimeLabel(subagent, now)}
        />
        <ChevronRight
          className="size-[9px] justify-self-end text-muted-foreground/50 group-hover:text-muted-foreground"
          aria-hidden
        />
      </span>
      <span
        data-subagent-excerpt
        title={excerpt}
        className="w-full truncate pr-4 pl-[26px] font-mono text-[9.5px] leading-[1.45] text-muted-foreground"
      >
        {excerpt}
      </span>
    </button>
  );
}

function WaveGlyph({ status }: { status: SubagentViewStatus }) {
  const tone = statusTone(status);
  return (
    <span
      className={cn(
        "flex size-[17px] shrink-0 items-center justify-center rounded-full",
        tone.chipBg,
      )}
      aria-hidden
    >
      {status === "running" ? (
        <span className="size-1.5 animate-pulse rounded-full bg-current" />
      ) : status === "failed" ? (
        <svg
          width="9"
          height="9"
          viewBox="0 0 12 12"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
        >
          <path d="M6 3.2v3.3M6 8.6v.2" />
        </svg>
      ) : status === "completed" ? (
        <Check className="size-[9px]" strokeWidth={2.4} />
      ) : (
        <Ban className="size-[9px]" strokeWidth={2.2} />
      )}
    </span>
  );
}

function RowGlyph({ subagent }: { subagent: SubagentView }) {
  if (isRunning(subagent)) {
    return (
      <span className="flex justify-center">
        <AgentOrb size={20} {...subagentOrbActivity(subagent)} aria-hidden />
      </span>
    );
  }
  const props = {
    className: "size-2.5 shrink-0 justify-self-center",
    "aria-hidden": true,
  } as const;
  if (subagent.status === "failed") {
    return (
      <X {...props} className={cn(props.className, "text-status-attention")} />
    );
  }
  if (subagent.status === "stopped" || subagent.status === "interrupted") {
    return (
      <Ban
        {...props}
        className={cn(props.className, "text-status-working/80")}
      />
    );
  }
  return (
    <Check {...props} className={cn(props.className, "text-status-open")} />
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
      className="inline-flex h-[17px] max-w-[96px] min-w-0 shrink-0 items-center gap-1 overflow-hidden rounded-[5px] border border-foreground/[0.08] bg-background/55 px-1.5 font-mono text-[8px] text-muted-foreground shadow-[inset_0_1px_0_rgba(255,255,255,0.025)]"
    >
      <span
        className="size-1 shrink-0 rounded-full bg-accent-ember/75"
        aria-hidden
      />
      <span className="truncate">{value}</span>
    </span>
  );
}

function plural(count: number, word: string): string {
  return `${count} ${word}${count === 1 ? "" : "s"}`;
}

/** "4 agents · 1 failed · Σ 1.1m tok" — prints only what the app knows. */
function waveMeta(subagents: readonly SubagentView[]): string {
  // Token aggregation is clock-independent; zero avoids consulting a
  // clock during render while still sharing the canonical rollup helper.
  const rollup = subagentGroupRollup(subagents, 0);
  const parts = [plural(subagents.length, "agent")];
  const failed = subagents.filter((s) => s.status === "failed").length;
  const halted = subagents.filter(
    (s) => s.status === "stopped" || s.status === "interrupted",
  ).length;
  if (rollup.activeCount > 0) parts.push(`${rollup.activeCount} running`);
  if (failed > 0) parts.push(`${failed} failed`);
  if (halted > 0) parts.push(`${halted} stopped`);
  if (rollup.totalTokens != null) {
    parts.push(`Σ ${formatCompactTokens(rollup.totalTokens)} tok`);
  }
  return parts.join(" · ");
}

function elapsedLabel(subagents: readonly SubagentView[], now: number): string {
  const elapsed = subagentGroupRollup(subagents, now).elapsedMs;
  return elapsed == null ? "" : formatElapsed(elapsed);
}

function rowTimeLabel(subagent: SubagentView, now: number): string {
  const elapsed = subagentElapsedMs(subagent, now);
  if (elapsed != null) return formatElapsed(elapsed);
  const tools = subagentToolCount(subagent);
  return `${tools} ${tools === 1 ? "tool" : "tools"}`;
}
