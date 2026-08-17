/**
 * The Subagents pane is Turn 2's watch surface. The transcript keeps only a
 * 32px work-log row; live detail, progress, and thread drill-in live here.
 */
import { Ban, Check, ChevronRight, X } from "lucide-react";
import { useMemo } from "react";

import { TickingText } from "@/components/chat/TickingText";
import { AgentOrb } from "@/components/ui/agent-orb";
import { subagentOrbActivity } from "@/lib/agent-chat/orb-activity";
import {
  formatElapsed,
  isRunning,
  subagentActivityLine,
  subagentElapsedMs,
  subagentGroupRollup,
  subagentRunItems,
  subagentToolCount,
} from "@/lib/agent-chat/subagents";
import type { ChatViewItem, SubagentView } from "@/lib/agent-chat/types";
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
  const runs = useMemo(() => subagentRunItems(messages), [messages]);
  const subagents = useMemo(() => {
    const byId = new Map<string, SubagentView>();
    for (const run of runs) {
      for (const subagent of run.subagents) byId.set(subagent.id, subagent);
    }
    return [...byId.values()];
  }, [runs]);
  const active = subagents.filter(isRunning);
  const finished = subagents.filter((subagent) => !isRunning(subagent));
  const progress =
    subagents.length === 0
      ? 0
      : Math.round((finished.length / subagents.length) * 100);

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

  return (
    <div
      data-testid="subagents-pane"
      className="h-full min-h-0 overflow-y-auto px-3 py-3.5"
    >
      <div className="flex h-5 items-center gap-2.5">
        <span className="shrink-0 font-mono text-[10px] text-foreground/80">
          {finished.length} / {subagents.length}
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
        aria-valuenow={finished.length}
        aria-label="Subagent progress"
      />

      {active.length > 0 && (
        <section
          className="mt-3"
          aria-label={`${active.length} live subagents`}
        >
          <div className="space-y-1.5">
            {active.map((subagent) => (
              <LiveSubagentCard
                key={subagent.id}
                subagent={subagent}
                onOpen={() => openThread(subagent.id)}
                canOpen={threadId != null}
              />
            ))}
          </div>
        </section>
      )}

      {finished.length > 0 && (
        <section className="mt-4" aria-labelledby="finished-subagents-heading">
          <h3
            id="finished-subagents-heading"
            className="mb-1 font-mono text-[9px] font-medium uppercase tracking-[0.12em] text-muted-foreground/65"
          >
            Finished · {finished.length}
          </h3>
          <div>
            {finished.map((subagent) => (
              <FinishedSubagentRow
                key={subagent.id}
                subagent={subagent}
                onOpen={() => openThread(subagent.id)}
                canOpen={threadId != null}
              />
            ))}
          </div>
        </section>
      )}
    </div>
  );
}

function LiveSubagentCard({
  subagent,
  onOpen,
  canOpen,
}: {
  subagent: SubagentView;
  onOpen: () => void;
  canOpen: boolean;
}) {
  const activity = subagentActivityLine(subagent);
  return (
    <div className="rounded-lg bg-foreground/[0.045] px-2.5 py-2.5">
      <div className="flex min-w-0 items-center gap-2">
        <AgentOrb size={20} {...subagentOrbActivity(subagent)} aria-hidden />
        <span className="min-w-0 flex-1 truncate text-[11px] font-semibold text-foreground/90">
          {subagent.name ?? subagent.agentType ?? "Subagent"}
        </span>
        <SubagentModelBadge model={subagent.model} />
      </div>
      <div className="mt-1.5 truncate pl-7 font-mono text-[10px] text-muted-foreground">
        {activity}
      </div>
      <div className="mt-2 ml-7 flex items-center gap-2">
        <button
          type="button"
          disabled={!canOpen}
          onClick={onOpen}
          className="flex items-center gap-0.5 text-[10px] font-medium text-foreground/75 hover:text-foreground disabled:cursor-default"
        >
          Open thread
          <ChevronRight className="size-3" aria-hidden />
        </button>
        <span className="ml-auto shrink-0 font-mono text-[9px] text-muted-foreground">
          {toolCountLabel(subagent)}
        </span>
      </div>
    </div>
  );
}

function FinishedSubagentRow({
  subagent,
  onOpen,
  canOpen,
}: {
  subagent: SubagentView;
  onOpen: () => void;
  canOpen: boolean;
}) {
  return (
    <button
      type="button"
      disabled={!canOpen}
      onClick={onOpen}
      className="group -mx-1.5 flex h-[26px] w-[calc(100%+0.75rem)] min-w-0 items-center gap-2 rounded-md px-1.5 text-left hover:bg-foreground/[0.04] disabled:cursor-default"
      aria-label={`Open ${subagent.name ?? subagent.agentType ?? "subagent"} thread`}
    >
      <FinishedGlyph subagent={subagent} />
      <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-foreground/75">
        {subagent.name ?? subagent.agentType ?? "Subagent"}
      </span>
      <SubagentModelBadge model={subagent.model} compact />
      <TickingText
        active={false}
        className="shrink-0 font-mono text-[9px] text-muted-foreground/80"
        compute={(now) => finishedTimeLabel(subagent, now)}
      />
      <ChevronRight
        className="size-3 shrink-0 text-muted-foreground/50 group-hover:text-muted-foreground"
        aria-hidden
      />
    </button>
  );
}

function FinishedGlyph({ subagent }: { subagent: SubagentView }) {
  const props = { className: "size-3 shrink-0", "aria-hidden": true } as const;
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
function SubagentModelBadge({
  model,
  compact = false,
}: {
  model?: string;
  compact?: boolean;
}) {
  const value = model?.trim();
  if (!value) return null;

  return (
    <span
      data-subagent-model={value}
      title={`Model: ${value}`}
      className={cn(
        "inline-flex min-w-0 shrink-0 items-center gap-1 overflow-hidden rounded-[5px] border border-foreground/[0.08] bg-background/55 font-mono text-muted-foreground shadow-[inset_0_1px_0_rgba(255,255,255,0.025)]",
        compact
          ? "h-[17px] max-w-[38%] px-1.5 text-[8px]"
          : "h-[18px] max-w-[48%] px-1.5 text-[9px]",
      )}
    >
      <span
        className="size-1 shrink-0 rounded-full bg-accent-ember/75"
        aria-hidden
      />
      <span className="truncate">{value}</span>
    </span>
  );
}

function elapsedLabel(subagents: readonly SubagentView[], now: number): string {
  const elapsed = subagentGroupRollup(subagents, now).elapsedMs;
  return elapsed == null ? "" : formatElapsed(elapsed);
}

function finishedTimeLabel(subagent: SubagentView, now: number): string {
  const elapsed = subagentElapsedMs(subagent, now);
  if (elapsed != null) return formatElapsed(elapsed);
  const tools = subagentToolCount(subagent);
  return `${tools} ${tools === 1 ? "tool" : "tools"}`;
}

function toolCountLabel(subagent: SubagentView): string {
  const tools = subagentToolCount(subagent);
  return `${tools} ${tools === 1 ? "tool" : "tools"}`;
}
