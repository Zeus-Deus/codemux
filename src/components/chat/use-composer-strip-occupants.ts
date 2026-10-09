import { useEffect, useRef, useState } from "react";

import { delegationPhaseSummary, delegationRow } from "@/lib/agent-chat/delegation";
import { subagentOrbActivity } from "@/lib/agent-chat/orb-activity";
import {
  delegationElapsedLabel,
  formatElapsed,
  runningDelegations,
  runningSubagentEntries,
  subagentActivityLine,
  subagentElapsedMs,
  type RunningSubagentEntry,
} from "@/lib/agent-chat/subagents";
import type {
  ChatViewItem,
  SubagentView,
  UsageLimitState,
  UserMessageItem,
} from "@/lib/agent-chat/types";
import {
  countdownIntervalMs,
  formatCountdown,
  formatResetTime,
  nextUsageLimitBoundary,
  usageLimitPhase,
  usageWindowLabel,
} from "@/lib/agent-chat/usage-limit";
import { resolveOrbState } from "@/lib/orb-state";
import { toast } from "@/lib/toast";
import type { AgentChatProviderKind } from "@/tauri/types";

import type { StripAction, StripOccupant, StripRow } from "./ComposerStrip";

/** How long the "just finished" row stays before it leaves the strip. */
export const FINISHED_FLASH_MS = 2500;

const JUMP_TITLE = "Jump to the Subagents card";

/**
 * Subagent occupant: running (one summary row, one row per subagent when
 * the strip is open) and the 2.5s finished flash.
 *
 * Counts every live subagent across every `subagent_run` card. While the
 * matching transcript row is on screen the occupant is withheld — the row
 * is the primary live surface, and the strip is only the off-screen tether
 * back to it. The finished flash plays only on an observed running → zero
 * transition, never on mount or a thread switch.
 */
export function useSubagentOccupant({
  messages,
  threadId,
  streaming,
  onJump,
}: {
  messages: ChatViewItem[];
  threadId: string | null;
  /** The thread's live-run flag. Once the run is over, background provider
   *  tasks stop counting as live, so the strip can't spin forever. */
  streaming: boolean;
  onJump: (cardId: string) => void;
}): StripOccupant | null {
  const entries = runningSubagentEntries(messages, streaming);
  const count = entries.length;

  const [finishedFlash, setFinishedFlash] = useState(false);
  const [transcriptRowVisible, setTranscriptRowVisible] = useState(false);
  const prevCountRef = useRef(count);
  const prevThreadRef = useRef(threadId);
  // By the time the >0 → 0 transition is observed `entries` is empty, so
  // the flash's Jump targets the last card captured while something ran.
  const lastRunningCardIdRef = useRef<string | null>(null);
  if (count > 0) {
    lastRunningCardIdRef.current = entries[entries.length - 1].cardId;
  }

  const runningCardIds = entries.map((entry) => entry.cardId).join("\u0000");
  useEffect(() => {
    if (count === 0 || typeof IntersectionObserver === "undefined") {
      setTranscriptRowVisible(false);
      return;
    }

    const ids = new Set(runningCardIds.split("\u0000"));
    let observed: Element | null = null;
    let intersection: IntersectionObserver | null = null;

    const findRow = () => {
      const marker = [...document.querySelectorAll("[data-subagent-run-id]")]
        .find((node) => ids.has(node.getAttribute("data-subagent-run-id") ?? ""));
      const row = marker?.closest("[data-subagent-card]") ?? null;
      if (row === observed) return;
      intersection?.disconnect();
      observed = row;
      if (!row) {
        setTranscriptRowVisible(false);
        return;
      }
      intersection = new IntersectionObserver(([entry]) => {
        setTranscriptRowVisible(entry?.isIntersecting === true);
      });
      intersection.observe(row);
    };

    findRow();
    const mutations = new MutationObserver(findRow);
    mutations.observe(document.body, { childList: true, subtree: true });
    return () => {
      intersection?.disconnect();
      mutations.disconnect();
    };
  }, [count, runningCardIds]);

  useEffect(() => {
    if (threadId !== prevThreadRef.current) {
      prevThreadRef.current = threadId;
      prevCountRef.current = count;
      lastRunningCardIdRef.current = null;
      setFinishedFlash(false);
      return;
    }

    const wasRunning = prevCountRef.current > 0;
    prevCountRef.current = count;

    if (wasRunning && count === 0) {
      setFinishedFlash(true);
      const timer = window.setTimeout(() => {
        setFinishedFlash(false);
      }, FINISHED_FLASH_MS);
      return () => window.clearTimeout(timer);
    }
    return undefined;
    // eslint-disable-next-line react-hooks/exhaustive-deps -- deliberately
    // keyed only on count/threadId; prevCountRef/prevThreadRef are refs.
  }, [count, threadId]);

  if (count === 0 && !finishedFlash) return null;
  if (count > 0 && transcriptRowVisible) return null;

  if (count === 0) {
    const cardId = lastRunningCardIdRef.current;
    const row: StripRow = {
      id: "subagents-finished",
      mark: { kind: "finished" },
      label: "Subagents finished",
      detail: "all tasks complete · results are in the thread",
      action: cardId
        ? { label: "Jump", title: JUMP_TITLE, onClick: () => onJump(cardId) }
        : null,
    };
    return { kind: "finished", summary: row, rows: [row] };
  }

  const primary = entries[0];
  const summary: StripRow = {
    id: "subagents-running",
    // This orb stands for the whole run, so it stays on the neutral
    // working state; each opened row owns its own activity-matched orb.
    mark: { kind: "orb" },
    label: `${count} subagent${count === 1 ? "" : "s"} running`,
    detail: runningActivityLabel(entries),
    elapsed: (now) => elapsedLabel(primary.subagent, now),
    action: {
      label: "Jump",
      title: JUMP_TITLE,
      onClick: () => onJump(primary.cardId),
    },
  };
  const rows = entries.map<StripRow>((entry) => {
    const activity = subagentActivityLine(entry.subagent);
    return {
      id: `subagent:${entry.subagent.id}`,
      mark: { kind: "orb", activity: subagentOrbActivity(entry.subagent) },
      label: entry.subagent.name ?? entry.subagent.agentType ?? "Subagent",
      detail: entry.fromLabel ? `${activity} · from ${entry.fromLabel}` : activity,
      elapsed: (now) => elapsedLabel(entry.subagent, now),
      action: {
        label: "Jump",
        title: JUMP_TITLE,
        onClick: () => onJump(entry.cardId),
      },
    };
  });
  return { kind: "running", summary, rows, live: true };
}

/**
 * Delegation occupant: tasks this chat handed to other agents that are still
 * running in their own tabs.
 *
 * Calm on purpose — no sweep, no orb. The parent is not working; it is
 * waiting to continue, and the copy says exactly that ("Continues when Codex
 * finishes"). While the parent itself streams, the row only notes that
 * reports will post here. Never withheld behind the transcript card: this is
 * the one place that explains why the chat will pick up again on its own.
 *
 * Stop's pending state copies the monitoring occupant: "Stopping…" holds
 * until the task leaves the running set (the backend's Stopped snapshot),
 * not until the command returns. "Stop all" rides on the first row, so it
 * survives opening the strip.
 */
export function useDelegationOccupant({
  messages,
  threadId,
  streaming,
  onOpen,
  onStop,
}: {
  messages: ChatViewItem[];
  threadId: string | null;
  /** The parent's own live-run flag. */
  streaming: boolean;
  onOpen: (childThreadId: string) => void;
  /** Swallows its own errors (`stopDelegatedTask`), so nothing unwinds. */
  onStop: (provider: AgentChatProviderKind, childThreadId: string) => void;
}): StripOccupant | null {
  // Row ids are unique per child, so a settled task's id can never match a
  // running row again; only a thread switch needs to drop them.
  const [stopping, setStopping] = useState<ReadonlySet<string>>(() => new Set());
  useEffect(() => {
    setStopping((current) => (current.size === 0 ? current : new Set()));
  }, [threadId]);

  const running = runningDelegations(messages);
  if (running.length === 0) return null;

  const models = running.map((view) => ({ view, row: delegationRow(null, view) }));
  const stop = (targets: typeof models) => {
    const fresh = targets.filter(({ view }) => !stopping.has(view.id));
    if (fresh.length === 0) return;
    setStopping((current) => new Set([...current, ...fresh.map(({ view }) => view.id)]));
    for (const { row } of fresh) {
      if (row.provider && row.childThreadId) onStop(row.provider, row.childThreadId);
    }
  };
  const count = models.length;
  const label = streaming
    ? `${count} delegated task${count === 1 ? "" : "s"} running`
    : count === 1
      ? `Continues when ${models[0].row.providerLabel} finishes`
      : `Continues when ${count} agents finish`;

  const taskRow = (model: (typeof models)[number], solo: boolean): StripRow => {
    const { view, row } = model;
    const pending = stopping.has(view.id);
    const thread = row.childThreadId;
    return {
      id: `delegation:${view.id}`,
      mark: { kind: "providers", providers: row.provider ? [row.provider] : [] },
      label: solo ? label : row.providerLabel,
      detail: `${row.title} · ${solo && streaming ? "reports post here" : row.line}`,
      elapsed: (now) => delegationElapsedLabel([view], now),
      secondaryAction: {
        label: pending ? "Stopping…" : "Stop",
        title: `Stop this task in ${row.providerLabel}`,
        tone: "quiet",
        testId: "composer-strip-delegation-stop",
        disabled: pending,
        onClick: () => stop([model]),
      },
      action: thread
        ? {
            label: "Open",
            title: `Open the ${row.providerLabel} chat`,
            testId: "composer-strip-delegation-open",
            onClick: () => onOpen(thread),
          }
        : null,
    };
  };

  if (count === 1) {
    const row = taskRow(models[0], true);
    return { kind: "delegation", summary: row, rows: [row] };
  }

  const rows = models.map(({ row }) => row);
  const blocked = rows.some((row) => row.phase === "waiting" || row.phase === "paused");
  const allStopping = running.every((view) => stopping.has(view.id));
  const header: StripRow = {
    id: "delegation:all",
    mark: { kind: "providers", providers: [...new Set(rows.flatMap((row) => row.provider ?? []))] },
    label,
    detail: streaming
      ? "reports post here"
      : blocked
        ? delegationPhaseSummary(rows)
        : "working in their tabs",
    elapsed: (now) => delegationElapsedLabel(running, now),
    secondaryAction: {
      label: allStopping ? "Stopping…" : "Stop all",
      title: `Stop all ${count} delegated tasks`,
      tone: "quiet",
      testId: "composer-strip-delegation-stop-all",
      disabled: allStopping,
      onClick: () => stop(models),
    },
    action: null,
    countsAsItem: false,
  };
  return {
    kind: "delegation",
    summary: header,
    rows: [header, ...models.map((model) => taskRow(model, false))],
  };
}

/**
 * Monitoring occupant: the pane reports `monitoring` (a settled thread
 * whose only live tasks are watch loops, or `codemux monitor start`).
 *
 * Stop's pending state resolves on the status leaving `monitoring`, not on
 * the command's promise — the recomputed status arrives as a separate
 * app-state emit, so "Stopping…" ends when the stop is visibly true. It
 * also unwinds on a thread switch and on a failed command.
 */
export function useMonitoringOccupant({
  monitoring,
  reason,
  threadId,
  onStop,
}: {
  monitoring: boolean;
  reason?: string | null;
  threadId: string | null;
  onStop: () => void | Promise<void>;
}): StripOccupant | null {
  const [stopping, setStopping] = useState(false);
  const prevThreadRef = useRef(threadId);

  useEffect(() => {
    if (threadId !== prevThreadRef.current) {
      prevThreadRef.current = threadId;
      setStopping(false);
      return;
    }
    if (!monitoring) setStopping(false);
  }, [threadId, monitoring]);

  if (!monitoring) return null;

  const handleStop = () => {
    if (stopping) return;
    setStopping(true);
    void Promise.resolve(onStop()).catch((error) => {
      console.error("[ComposerStrip] monitoring stop failed", error);
      setStopping(false);
    });
  };

  const row: StripRow = {
    id: "monitoring",
    mark: { kind: "monitoring" },
    label: "Monitoring",
    detail: reason || "watching in the background",
    action: {
      label: stopping ? "Stopping…" : "Stop",
      disabled: stopping,
      onClick: handleStop,
    },
  };
  return { kind: "monitoring", summary: row, rows: [row] };
}

/** Longest single wait the usage row schedules; it re-arms after. */
const MAX_TIMER_MS = 24 * 60 * 60_000;

/**
 * Usage-limit occupant: the provider stopped the run on a subscription
 * limit and nothing has been sent since. The row counts down to the
 * automatic resume (or to the reset when none is armed) and carries the
 * one way to resume by hand; the transcript's `usage_limit` row is only
 * the record.
 *
 * The countdown text repaints itself (`liveDetail`); React re-renders only
 * at phase boundaries — the resume or reset instant, the start of the
 * per-second window, and the end of the "Resuming…" grace.
 */
export function useUsageLimitOccupant({
  usageLimit,
  threadId,
  streaming,
  onResume,
  onCancel,
}: {
  usageLimit: UsageLimitState | null;
  threadId: string | null;
  /** Live run or a send in flight: a turn is already answering the limit. */
  streaming: boolean;
  onResume: () => Promise<void>;
  onCancel: () => Promise<void>;
}): StripOccupant | null {
  const [clock, setClock] = useState(() => Date.now());
  const [pending, setPending] = useState<"resume" | "cancel" | null>(null);

  useEffect(() => {
    if (!usageLimit) return;
    const now = Date.now();
    // A limit that arrives after a long idle must not be judged against a
    // stale clock.
    if (Math.abs(now - clock) > 1_000) {
      setClock(now);
      return;
    }
    const next = nextUsageLimitBoundary(usageLimit, now);
    if (next === null) return;
    const id = window.setTimeout(
      () => setClock(Date.now()),
      Math.min(MAX_TIMER_MS, Math.max(0, next - now) + 25),
    );
    return () => window.clearTimeout(id);
  }, [usageLimit, clock]);

  // A new limit, a disarm, or a thread switch settles any in-flight click.
  useEffect(() => {
    setPending(null);
  }, [usageLimit, threadId]);

  if (!usageLimit || streaming) return null;

  const run = (kind: "resume" | "cancel") => {
    if (pending) return;
    setPending(kind);
    const action = kind === "resume" ? onResume : onCancel;
    void Promise.resolve()
      .then(action)
      .catch((error: unknown) => {
        console.error("[ComposerStrip] usage-limit action failed", error);
        toast.error(
          kind === "resume"
            ? "Couldn't resume the run"
            : "Couldn't cancel the automatic resume",
          { description: error instanceof Error ? error.message : String(error) },
        );
      })
      .finally(() => setPending(null));
  };

  const busy = pending !== null;
  const tryNow: StripAction = {
    label: "Try now",
    title: "Resume now instead of waiting for the reset",
    testId: "composer-strip-usage-try-now",
    disabled: busy,
    onClick: () => run("resume"),
  };
  const resume: StripAction = {
    label: "Resume",
    title: "Continue the run the usage limit stopped",
    tone: "solid",
    testId: "composer-strip-usage-resume",
    disabled: busy,
    onClick: () => run("resume"),
  };
  const windowLabel = usageWindowLabel(usageLimit.window);
  const phase = usageLimitPhase(usageLimit, clock);

  let row: StripRow;
  switch (phase.kind) {
    case "armed":
    case "waiting": {
      const at = phase.at;
      const verb =
        phase.kind === "armed" ? "resuming automatically in" : "resets in";
      row = {
        id: "usage-limit",
        mark: { kind: "usage" },
        label: "Usage limit",
        liveDetail: {
          compute: (now) => `${verb} ${formatCountdown(at - now)}`,
          intervalMs: countdownIntervalMs(at, clock),
          testId: "composer-strip-usage-countdown",
        },
        meta: `at ${formatResetTime(at, clock)}`,
        secondaryAction:
          phase.kind === "armed"
            ? {
                label: "Cancel",
                title: "Don't resume automatically",
                tone: "quiet",
                testId: "composer-strip-usage-cancel",
                disabled: busy,
                onClick: () => run("cancel"),
              }
            : null,
        action: tryNow,
      };
      break;
    }
    case "resuming":
      row = {
        id: "usage-limit",
        mark: { kind: "usage" },
        label: "Resuming…",
        detail: "the usage limit has reset",
      };
      break;
    case "reset":
    case "reached":
      row = {
        id: "usage-limit",
        mark: { kind: "usage" },
        label:
          phase.kind === "reset" ? "Usage limit has reset" : "Usage limit reached",
        detail: windowLabel,
        action: resume,
      };
      break;
  }
  return { kind: "usage", summary: row, rows: [row] };
}

/** Follow-ups parked behind the active turn, oldest first. */
export function queuedMessages(messages: ChatViewItem[]): UserMessageItem[] {
  const queued: UserMessageItem[] = [];
  for (const m of messages) {
    if (m.kind === "user_message" && m.queued) queued.push(m);
  }
  return queued;
}

/** Queued-message occupant. Edit hands the text back to the composer
 *  (cancelling the queued turn), exactly like the transcript's cancel. */
export function queuedOccupant(
  queued: UserMessageItem[],
  onEdit?: (queuedId: string, text: string) => void,
): StripOccupant | null {
  if (queued.length === 0) return null;
  const rows = queued.map<StripRow>((m) => {
    const queuedId = m.queued!.queuedId;
    return {
      id: `queued:${queuedId}`,
      mark: { kind: "queued" },
      label: "Queued",
      detail: m.text.replace(/\s+/g, " ").trim(),
      action: onEdit
        ? {
            label: "Edit",
            title: "Move this message back into the composer",
            onClick: () => onEdit(queuedId, m.text),
          }
        : null,
    };
  });
  return { kind: "queued", summary: rows[0], rows };
}

const SESSION_ERROR_PREFIX = /^Session error:\s*/;

/** Session-error occupant: the thread's latest row (ignoring turn
 *  boundaries) is a terminal runtime error and nothing has run since. */
export function sessionErrorOccupant(
  messages: ChatViewItem[],
  streaming: boolean,
): StripOccupant | null {
  if (streaming) return null;
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (m.kind === "turn_ended") continue;
    if (m.kind !== "runtime_notice" || m.severity !== "error") return null;
    const row: StripRow = {
      id: `error:${m.id}`,
      mark: { kind: "error" },
      label: "Session error",
      detail: m.message.replace(SESSION_ERROR_PREFIX, ""),
    };
    return { kind: "error", summary: row, rows: [row] };
  }
  return null;
}

/**
 * What kind of busy the run is, e.g. "solving · connecting" — the same
 * orb-state vocabulary the rows animate, deduped and capped at three.
 * Deliberately not gated on the "Match the orb to the activity" setting:
 * that governs the animation, not what the app knows.
 */
function runningActivityLabel(entries: RunningSubagentEntry[]): string {
  const states = new Set<string>();
  for (const entry of entries) {
    states.add(resolveOrbState(subagentOrbActivity(entry.subagent)));
    if (states.size >= 3) break;
  }
  return [...states].join(" · ");
}

/** Empty when nothing is derivable, so no number is fabricated. */
function elapsedLabel(subagent: SubagentView, now: number): string {
  const ms = subagentElapsedMs(subagent, now);
  return ms != null ? formatElapsed(ms) : "";
}
