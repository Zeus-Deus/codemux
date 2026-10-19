import { ArrowUpRight, Check, GitMerge, ListPlus } from "lucide-react";
import { memo, useContext, useMemo, useState } from "react";

import {
  DELEGATION_PHASE_LABEL,
  isLivePhase,
  providerKindOf,
  type DelegationPhase,
} from "@/lib/agent-chat/delegation";
import { defaultModelId, fallbackModelLabel } from "@/lib/agent-chat/capability-defaults";
import {
  cachedSuggestedTasks,
  defaultFollowUpRun,
  doItHereTurn,
  mergeableCount,
  mergeSummary,
  resolveFollowUpEffort,
  startedTaskState,
  type FollowUpRun,
  type StartedTaskState,
  type SuggestedTask,
} from "@/lib/agent-chat/follow-ups";
import { codexTurnPermissionOverride } from "@/lib/agent-chat/mode-compatibility";
import { sessionProviderLabel } from "@/lib/agent-chat/session-mentions";
import { statusTone } from "@/lib/agent-chat/subagents";
import type { SubagentView, SubagentViewStatus, ToolCallItem } from "@/lib/agent-chat/types";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import {
  selectCapabilities,
  selectModel,
  useProviderCapabilities,
} from "@/stores/provider-capabilities-store";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import {
  agentChatSendTurn,
  followUpMergeAll,
  followUpStart,
  type FollowUpMergeResult,
  type FollowUpProvider,
  type FollowUpTaskRow,
} from "@/tauri/commands";
import type { AgentChatProviderKind, ChatModelInfo } from "@/tauri/types";

import { AsyncQuestionThreadContext } from "./AsyncQuestionPanel";
import { useChatProvider } from "./chat-provider-context";
import { openDelegatedChat } from "./delegation-actions";
import { useFollowUpProviders, useFollowUpRows, useFollowUpStore } from "./follow-up-store";
import { MultiProviderModelPicker } from "./pickers/MultiProviderModelPicker";
import { ReasoningPicker } from "./pickers/ReasoningPicker";
import { ProviderLogo } from "./provider-logo";

/** Same tones as the delegation card, so a started task reads the same. */
const PHASE_TONE: Record<DelegationPhase, SubagentViewStatus> = {
  starting: "pending",
  working: "running",
  waiting: "failed",
  paused: "interrupted",
  done: "completed",
  failed: "failed",
  stopped: "stopped",
};

const PHASE_LABEL: Record<DelegationPhase, string> = {
  ...DELEGATION_PHASE_LABEL,
  waiting: "Waiting for your answer",
};

const PHASE_DOT: Record<Exclude<DelegationPhase, "starting">, string> = {
  working: "bg-status-working",
  waiting: "bg-status-attention motion-safe:animate-pulse",
  paused: "bg-status-working/60",
  done: "bg-status-open",
  failed: "bg-status-attention",
  stopped: "bg-muted-foreground/60",
};

const FALLBACK_PROVIDERS: FollowUpProvider[] = [
  { id: "claude", label: "Claude", installed: true },
  { id: "codex", label: "Codex", installed: true },
];

type Capabilities = ReturnType<typeof useProviderCapabilities.getState>;

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function modelInfo(
  caps: Capabilities,
  provider: AgentChatProviderKind | null,
  model: string | null,
): ChatModelInfo | null {
  return provider ? selectModel(selectCapabilities(caps, provider), model) : null;
}

/** "Opus 4.8 · High": the model, then the effort when it has one. */
function runLabel(
  caps: Capabilities,
  provider: AgentChatProviderKind | null,
  model: string | null,
  effort: string | null,
): string {
  const info = modelInfo(caps, provider, model);
  const name =
    info?.label ?? (model ? fallbackModelLabel(model) : sessionProviderLabel(provider ?? ""));
  const effortLabel = effort
    ? (provider ? selectCapabilities(caps, provider)?.effort_label_map[effort] : null) ?? effort
    : null;
  return effortLabel ? `${name} · ${effortLabel}` : name;
}

/** Only the effort section of the composer's reasoning picker applies:
 *  the backend starts a follow-up with a model and an effort, nothing else. */
function effortOnly(model: ChatModelInfo): ChatModelInfo {
  return {
    ...model,
    prompt_injected_effort_levels: [],
    context_window_options: [],
    service_tiers: [],
    supports_fast_mode: false,
  };
}

/**
 * Follow-up tasks the agent suggested with `suggest_tasks`. Nothing runs
 * until the user acts: Start runs a task as its own workspace on a new git
 * worktree branched from this workspace's latest commit; Do it here asks
 * this chat to do it now; Merge all brings finished branches back.
 *
 * Each task runs on the model and effort its row shows: the parent chat's
 * own by default, or the suggested provider's default model.
 *
 * Without a writable thread (a passive snapshot, a drill-in) it renders the
 * list read-only.
 */
export const SuggestedTasksCard = memo(function SuggestedTasksCard({
  call,
  views,
}: {
  call: ToolCallItem;
  views: ReadonlyArray<SubagentView | null>;
}) {
  const threadId = useContext(AsyncQuestionThreadContext);
  const chatProvider = useChatProvider();
  const chatModel = useAgentChatStore((s) => (threadId ? (s.threads[threadId]?.model ?? null) : null));
  const chatEffort = useAgentChatStore((s) =>
    threadId ? (s.threads[threadId]?.effort ?? null) : null,
  );
  const caps = useProviderCapabilities();
  const tasks = cachedSuggestedTasks(call) ?? [];
  const allRows = useFollowUpRows(threadId);
  const providers = useFollowUpProviders() ?? FALLBACK_PROVIDERS;
  const allowed = useMemo(
    () =>
      providers
        .map((provider) => providerKindOf(provider.id))
        .filter((kind): kind is AgentChatProviderKind => kind != null),
    [providers],
  );
  const rowsByIndex = useMemo(() => {
    const map = new Map<number, FollowUpTaskRow>();
    for (const row of allRows) if (row.tool_call_id === call.tool_use_id) map.set(row.index, row);
    return map;
  }, [allRows, call.tool_use_id]);
  const [choice, setChoice] = useState<Record<number, FollowUpRun>>({});
  const [busy, setBusy] = useState<ReadonlySet<number>>(new Set());
  const [merging, setMerging] = useState(false);
  const [mergeResult, setMergeResult] = useState<FollowUpMergeResult | null>(null);
  const [startingAll, setStartingAll] = useState(false);

  const defaultModel = (provider: AgentChatProviderKind) =>
    selectCapabilities(caps, provider)?.models[0]?.id ?? defaultModelId(provider);
  const runFor = (index: number): FollowUpRun =>
    choice[index] ??
    defaultFollowUpRun({
      task: tasks[index],
      allowed,
      chatProvider,
      chatModel,
      chatEffort,
      defaultModel,
    });
  /** What Start sends: the effort pinned to one the model accepts. */
  const resolvedRun = (index: number): FollowUpRun => {
    const run = runFor(index);
    return { ...run, effort: resolveFollowUpEffort(modelInfo(caps, run.provider, run.model), run.effort) };
  };
  const started = (index: number) => rowsByIndex.has(index) || views[index] != null;
  const notStarted = tasks.map((_, index) => index).filter((index) => !started(index));
  const writable = threadId != null;

  const start = async (index: number): Promise<boolean> => {
    if (!threadId || busy.has(index) || started(index)) return false;
    setBusy((prev) => new Set(prev).add(index));
    const run = resolvedRun(index);
    try {
      await followUpStart({
        parentThreadId: threadId,
        toolCallId: call.tool_use_id,
        index,
        provider: run.provider,
        model: run.model,
        effort: run.effort,
      });
      await useFollowUpStore.getState().refresh(threadId);
      return true;
    } catch (error) {
      toast.error(`Couldn't start “${tasks[index].title}”`, { description: errorText(error) });
      return false;
    } finally {
      setBusy((prev) => {
        const next = new Set(prev);
        next.delete(index);
        return next;
      });
    }
  };

  const startAll = async () => {
    if (startingAll) return;
    setStartingAll(true);
    // One at a time: each makes a worktree, and the backend refuses a
    // second start of the same suggestion anyway.
    for (const index of notStarted) await start(index);
    setStartingAll(false);
  };

  const doItHere = async (index: number) => {
    if (!threadId || !chatProvider) return;
    const turn = doItHereTurn(tasks[index]);
    const slice = useAgentChatStore.getState().threads[threadId];
    try {
      await agentChatSendTurn(chatProvider, {
        thread_id: threadId,
        text: turn.text,
        display_text: turn.displayText,
        model_override: null,
        effort_override: null,
        // Runs in the chat's current mode, like a message sent by hand.
        permission_mode_override: codexTurnPermissionOverride(
          chatProvider,
          slice?.mode ?? "default",
          slice?.permissionMode ?? null,
        ),
      });
    } catch (error) {
      toast.error("Couldn't send the task to this chat", { description: errorText(error) });
    }
  };

  const mergeAll = async () => {
    if (!threadId || merging) return;
    setMerging(true);
    try {
      setMergeResult(await followUpMergeAll(threadId));
    } catch (error) {
      setMergeResult(null);
      toast.error("Couldn't merge", { description: errorText(error) });
    } finally {
      setMerging(false);
    }
  };

  if (tasks.length === 0) return null;
  // Merge all takes every finished child of this chat, not only this card's.
  const mergeable = mergeableCount(allRows);
  const summary = mergeResult ? mergeSummary(mergeResult) : null;
  const count = tasks.length;

  return (
    <div
      data-testid="suggested-tasks-card"
      className="overflow-hidden rounded-md border border-hairline-strong bg-surface-1"
    >
      {/* min-h keeps the header still when its buttons come and go. */}
      <div className="flex min-h-10 items-center gap-2.5 border-b border-hairline px-3 py-2">
        <ListPlus className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
        <span className="min-w-0 flex-1 truncate text-label font-medium text-foreground/90">
          {count} suggested task{count === 1 ? "" : "s"}
        </span>
        {writable && mergeable > 0 && (
          <Button
            variant="ghost"
            size="xs"
            data-testid="suggested-tasks-merge-all"
            disabled={merging}
            onClick={() => void mergeAll()}
            title={`Merge this chat's ${mergeable} finished branch${mergeable === 1 ? "" : "es"} into this workspace`}
            className="text-muted-foreground"
          >
            {merging ? <Spinner className="size-3" aria-hidden /> : <GitMerge aria-hidden />}
            {merging ? "Merging…" : "Merge all"}
          </Button>
        )}
        {writable && notStarted.length > 1 && (
          <Button
            variant="outline"
            size="xs"
            data-testid="suggested-tasks-start-all"
            disabled={startingAll}
            onClick={() => void startAll()}
            title="Start every task in its own worktree, each on the model its row shows"
          >
            {startingAll && <Spinner className="size-3" aria-hidden />}
            {startingAll ? "Starting…" : "Start all"}
          </Button>
        )}
      </div>
      {summary && (
        <p
          data-testid="suggested-tasks-merge-result"
          role="status"
          className={cn(
            "border-b border-hairline px-3 py-2 text-label",
            summary.failed ? "text-status-attention" : "text-muted-foreground",
          )}
        >
          {summary.text}
        </p>
      )}
      <ul className="flex flex-col divide-y divide-hairline">
        {tasks.map((task, index) => {
          const run = resolvedRun(index);
          return (
            <SuggestedTaskRow
              key={index}
              task={task}
              state={startedTaskState(rowsByIndex.get(index) ?? null, views[index] ?? null)}
              run={run}
              runLabel={runLabel(caps, run.provider, run.model, run.effort)}
              allowed={allowed}
              onRun={(next) => setChoice((prev) => ({ ...prev, [index]: next }))}
              starting={busy.has(index)}
              writable={writable}
              canDoHere={writable && chatProvider != null}
              onStart={() => void start(index)}
              onDoHere={() => void doItHere(index)}
            />
          );
        })}
      </ul>
    </div>
  );
});

function SuggestedTaskRow({
  task,
  state,
  run,
  runLabel: label,
  allowed,
  onRun,
  starting,
  writable,
  canDoHere,
  onStart,
  onDoHere,
}: {
  task: SuggestedTask;
  state: StartedTaskState | null;
  run: FollowUpRun;
  runLabel: string;
  allowed: readonly AgentChatProviderKind[];
  onRun: (run: FollowUpRun) => void;
  starting: boolean;
  writable: boolean;
  canDoHere: boolean;
  onStart: () => void;
  onDoHere: () => void;
}) {
  const [expanded, setExpanded] = useState(false);
  return (
    <li
      data-testid="suggested-task-row"
      data-state={state ? state.phase : "suggested"}
      className="flex flex-col gap-1.5 px-3 py-2.5"
    >
      <div className="min-w-0">
        <p className="truncate text-body-sm font-medium text-foreground" title={task.title}>
          {task.title}
        </p>
        <button
          type="button"
          onClick={() => setExpanded((open) => !open)}
          aria-expanded={expanded}
          className={cn(
            "mt-0.5 block w-full rounded-sm text-left text-label text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60",
            expanded ? "whitespace-pre-wrap" : "line-clamp-2",
          )}
        >
          {task.task}
        </button>
      </div>
      {state ? (
        <StartedStatus state={state} />
      ) : (
        writable && (
          <div className="flex min-w-0 items-center gap-1">
            <RunPicker run={run} allowed={allowed} onRun={onRun} disabled={starting} />
            {canDoHere && (
              <Button
                variant="ghost"
                size="xs"
                data-testid="suggested-task-do-here"
                onClick={onDoHere}
                title="Ask this chat to do it now"
                className="text-muted-foreground"
              >
                Do it here
              </Button>
            )}
            <Button
              variant="outline"
              size="xs"
              data-testid="suggested-task-start"
              disabled={starting}
              onClick={onStart}
              title={`Start in its own worktree with ${label}`}
            >
              {starting && <Spinner className="size-3" aria-hidden />}
              {starting ? "Starting…" : "Start"}
            </Button>
          </div>
        )
      )}
    </li>
  );
}

/**
 * The composer's own model and effort pickers, side by side like its
 * footer. Changing provider resets effort to the new model's default; a
 * model change within a provider keeps it while the model accepts it.
 */
function RunPicker({
  run,
  allowed,
  onRun,
  disabled,
}: {
  run: FollowUpRun;
  allowed: readonly AgentChatProviderKind[];
  onRun: (run: FollowUpRun) => void;
  disabled: boolean;
}) {
  const caps = useProviderCapabilities();
  const providerCaps = selectCapabilities(caps, run.provider);
  const model = selectModel(providerCaps, run.model);
  return (
    <div
      data-testid="suggested-task-run"
      // The hover plate's padding sits outside the text column, so the
      // logo lines up with the title above.
      className="-ml-2 flex min-w-0 flex-1 items-center"
    >
      <MultiProviderModelPicker
        provider={run.provider}
        model={run.model}
        allowedProviders={allowed}
        customAgentsAllowed={false}
        disabled={disabled}
        onProviderModelChange={(provider, next) =>
          onRun({
            provider,
            model: next,
            effort: provider === run.provider ? run.effort : null,
          })
        }
      />
      <ReasoningPicker
        model={model ? effortOnly(model) : null}
        effortValue={run.effort}
        contextWindowValue={null}
        labelMap={providerCaps?.effort_label_map ?? {}}
        ultrathinkInBodyText={false}
        serviceTier="default"
        onEffortChange={(effort) => onRun({ ...run, effort })}
        onContextWindowChange={() => undefined}
        onServiceTierChange={() => undefined}
        disabled={disabled}
        withSeparator
      />
    </div>
  );
}

function StartedStatus({ state }: { state: StartedTaskState }) {
  const caps = useProviderCapabilities();
  const tone = statusTone(PHASE_TONE[state.phase]);
  const open =
    state.childThreadId != null ? () => void openDelegatedChat(state.childThreadId!) : null;
  const ranOn = state.provider ? runLabel(caps, state.provider, state.model, state.effort) : null;
  const detail = [
    state.branch,
    state.phase === "done" ? state.diff : null,
    state.merged ? "merged" : null,
    state.phase === "done" && state.uncommitted > 0
      ? `${state.uncommitted} uncommitted, not mergeable`
      : null,
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <div data-testid="suggested-task-status" className="flex min-w-0 items-center gap-2">
      <Badge variant="secondary" className={tone.chipBg}>
        {state.phase === "starting" ? (
          <Spinner className="size-3" aria-hidden />
        ) : state.merged ? (
          <Check aria-hidden />
        ) : (
          <span className={cn("size-1.5 shrink-0 rounded-full", PHASE_DOT[state.phase])} aria-hidden />
        )}
        {state.merged ? "Merged" : PHASE_LABEL[state.phase]}
      </Badge>
      {state.provider && ranOn && (
        <span
          data-testid="suggested-task-model"
          className="inline-flex max-w-[45%] shrink-0 items-center gap-1 text-label text-muted-foreground"
          title={`Runs on ${sessionProviderLabel(state.provider)} · ${ranOn}`}
        >
          <ProviderLogo provider={state.provider} className="size-3.5" />
          <span className="truncate">{ranOn}</span>
        </span>
      )}
      <span
        className={cn(
          "min-w-0 flex-1 truncate text-label",
          state.phase === "failed" ? "text-status-attention" : "text-muted-foreground",
        )}
        title={[state.line, detail].filter(Boolean).join("\n")}
      >
        {isLivePhase(state.phase) || state.phase === "failed" ? state.line : detail || state.line}
      </span>
      {open && (
        <Button
          variant="ghost"
          size="xs"
          data-testid="suggested-task-open"
          onClick={open}
          title="Open its workspace"
          className="text-muted-foreground"
        >
          Open
          <ArrowUpRight aria-hidden />
        </Button>
      )}
    </div>
  );
}
