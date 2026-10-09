import { ArrowUpRight, Bot, LoaderCircle } from "lucide-react";
import { memo, useEffect, useState } from "react";

import {
  DELEGATION_PHASE_LABEL,
  delegationPhaseSummary,
  delegationRow,
  isLivePhase,
  type DelegationPhase,
  type DelegationRow,
} from "@/lib/agent-chat/delegation";
import { delegationElapsedLabel, statusTone } from "@/lib/agent-chat/subagents";
import type { SubagentViewStatus } from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";
import type { AgentChatProviderKind } from "@/tauri/types";

import { openDelegatedChat, stopDelegatedTask } from "./delegation-actions";
import { ProviderLogo } from "./provider-logo";
import { TickingText } from "./TickingText";
import type { DelegationEntry } from "./transcript-slots";

/** Which `statusTone` each phase borrows: the same colours every other
 *  subagent surface uses. Waiting reads as attention, like "Needs you". */
const PHASE_TONE: Record<DelegationPhase, SubagentViewStatus> = {
  starting: "pending",
  working: "running",
  waiting: "failed",
  paused: "interrupted",
  done: "completed",
  failed: "failed",
  stopped: "stopped",
};

/** The pill's dot. Steady while working (the app pulses only for "look at
 *  me"), pulsing while the child waits on the user. */
const PHASE_DOT: Record<Exclude<DelegationPhase, "starting">, string> = {
  working: "bg-status-working",
  waiting: "bg-status-attention motion-safe:animate-pulse",
  paused: "bg-status-working/60",
  done: "bg-status-open",
  failed: "bg-status-attention",
  stopped: "bg-muted-foreground/60",
};

const ROW_ACTION =
  "inline-flex h-5 items-center gap-0.5 rounded-sm px-1.5 text-label font-medium text-muted-foreground transition-colors duration-100 hover:bg-surface-3 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60 disabled:pointer-events-none disabled:opacity-50";

interface RowModel {
  entry: DelegationEntry;
  row: DelegationRow;
}

/**
 * Tasks this chat handed to other agents, at the point it handed them off.
 *
 * One card per contiguous run of delegations, never folded into the work
 * log: each child is a separate chat the user may want to watch, answer or
 * stop, and the card is how they get there. Each row says who (provider
 * mark, model · effort), what (the task title), and where it stands (pill,
 * a live timer that freezes when it settles, one activity or result line).
 * The row opens the child's tab; Stop shows only while it can still apply.
 */
export const DelegationCard = memo(function DelegationCard({
  entries,
}: {
  entries: DelegationEntry[];
}) {
  const rows: RowModel[] = entries.map((entry) => ({
    entry,
    row: delegationRow(entry.call, entry.view),
  }));
  if (rows.length === 0) return null;
  return (
    <div
      data-testid="delegation-card"
      className="overflow-hidden rounded-md border border-hairline-strong bg-surface-1"
    >
      {rows.length > 1 && <GroupHeader rows={rows} />}
      <ul className="flex flex-col divide-y divide-hairline">
        {rows.map((model) => (
          <DelegationRowView key={model.entry.key} {...model} />
        ))}
      </ul>
    </div>
  );
});

function GroupHeader({ rows }: { rows: RowModel[] }) {
  const providers = [...new Set(rows.map(({ row }) => row.provider))];
  return (
    <div
      data-testid="delegation-group-header"
      className="flex items-center gap-2.5 border-b border-hairline px-3 py-2"
    >
      <LogoStack providers={providers} />
      <span className="min-w-0 flex-1 truncate text-label text-muted-foreground">
        <span className="font-medium text-foreground/90">{rows.length} agents</span>
        {` · ${delegationPhaseSummary(rows.map(({ row }) => row))}`}
      </span>
      <TickingText
        active={rows.some(({ row }) => isLivePhase(row.phase))}
        className="shrink-0 font-mono text-caption text-muted-foreground/70"
        compute={(now) => delegationElapsedLabel(rows.map(({ entry }) => entry.view), now)}
      />
    </div>
  );
}

function DelegationRowView({ entry, row }: RowModel) {
  const [stopping, setStopping] = useState(false);
  // "Stopping…" holds until the backend's Stopped snapshot lands, not until
  // the command returns — that is when the stop is visibly true.
  useEffect(() => {
    if (!row.live) setStopping(false);
  }, [row.live]);

  const thread = row.childThreadId;
  const provider = row.provider;
  const open = thread ? () => void openDelegatedChat(thread) : null;
  const stop =
    row.live && thread && provider
      ? () => {
          if (stopping) return;
          setStopping(true);
          void stopDelegatedTask(provider, thread);
        }
      : null;
  const meta = [row.providerLabel, row.model, row.effort].filter(Boolean).join(" · ");
  const tone = statusTone(PHASE_TONE[row.phase]);

  return (
    <li
      data-testid="delegation-row"
      data-phase={row.phase}
      // On the row, not the text: the open overlay covers the text column,
      // and hovering it shows the nearest ancestor's title.
      title={[row.title, meta, row.line].filter(Boolean).join("\n")}
      className={cn(
        "relative flex items-center gap-3 px-3 py-2.5 transition-colors duration-150",
        open && "hover:bg-surface-2",
      )}
    >
      {open && (
        // The whole row opens the child. Only the Stop / Open buttons take
        // clicks above this layer; the positioned column they sit in lets
        // every other click (timer, pill, gaps) fall through to it.
        <button
          type="button"
          aria-label={`Open ${row.providerLabel} · ${row.title}`}
          onClick={open}
          className="absolute inset-0 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/60"
        />
      )}
      <ProviderTile provider={provider} label={row.providerLabel} />
      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 items-baseline gap-2">
          <span className="min-w-0 truncate text-body-sm font-medium text-foreground">
            {row.title}
          </span>
          <span
            data-testid="delegation-row-meta"
            className="max-w-[55%] shrink-0 truncate font-mono text-caption text-muted-foreground/80"
          >
            {meta}
          </span>
        </div>
        <p
          data-testid="delegation-row-line"
          className={cn(
            "mt-0.5 truncate text-label",
            row.phase === "failed"
              ? "text-status-attention"
              : "text-muted-foreground",
          )}
        >
          {row.line}
        </p>
      </div>
      <div className="pointer-events-none relative flex shrink-0 flex-col items-end gap-1">
        <div className="flex items-center gap-2">
          <TickingText
            active={isLivePhase(row.phase)}
            testId="delegation-row-elapsed"
            className="font-mono text-caption text-muted-foreground/70"
            compute={(now) => delegationElapsedLabel([entry.view], now)}
          />
          <span
            data-testid="delegation-row-status"
            className={cn(
              "inline-flex h-[18px] items-center gap-1 rounded-sm px-1.5 text-caption font-medium whitespace-nowrap",
              tone.chipBg,
            )}
          >
            {row.phase === "starting" ? (
              <LoaderCircle className="size-3 motion-safe:animate-spin" aria-hidden />
            ) : (
              <span
                className={cn("size-1.5 shrink-0 rounded-full", PHASE_DOT[row.phase])}
                aria-hidden
              />
            )}
            {DELEGATION_PHASE_LABEL[row.phase]}
          </span>
        </div>
        {(stop || open) && (
          <div className="pointer-events-auto flex items-center gap-0.5">
            {stop && (
              <button
                type="button"
                data-testid="delegation-row-stop"
                disabled={stopping}
                onClick={stop}
                title={`Stop this task in ${row.providerLabel}`}
                className={ROW_ACTION}
              >
                {stopping ? "Stopping…" : "Stop"}
              </button>
            )}
            {open && (
              <button
                type="button"
                data-testid="delegation-row-open"
                onClick={open}
                title={`Open the ${row.providerLabel} chat`}
                className={ROW_ACTION}
              >
                Open
                <ArrowUpRight className="size-3" aria-hidden />
              </button>
            )}
          </div>
        )}
      </div>
    </li>
  );
}

/** The child's provider mark on a quiet tile, the row's anchor. */
function ProviderTile({
  provider,
  label,
}: {
  provider: AgentChatProviderKind | null;
  label: string;
}) {
  return (
    <span
      title={label}
      className="flex size-7 shrink-0 items-center justify-center rounded-md border border-hairline bg-background/60"
    >
      {provider ? (
        <ProviderLogo provider={provider} className="size-4" />
      ) : (
        <Bot className="size-3.5 text-muted-foreground" aria-hidden />
      )}
    </span>
  );
}

/** Up to three overlapping provider marks for a group header. */
function LogoStack({ providers }: { providers: Array<AgentChatProviderKind | null> }) {
  return (
    <span className="flex shrink-0 items-center -space-x-1.5" aria-hidden>
      {providers.slice(0, 3).map((provider) => (
        <span
          key={provider ?? "unknown"}
          className="flex size-5 items-center justify-center rounded-full border border-hairline bg-background ring-2 ring-background"
        >
          {provider ? (
            <ProviderLogo provider={provider} className="size-3" />
          ) : (
            <Bot className="size-3 text-muted-foreground" aria-hidden />
          )}
        </span>
      ))}
    </span>
  );
}
