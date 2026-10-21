import {
  Bot,
  Check,
  ChevronDown,
  ChevronRight,
  CircleDashed,
  CircleX,
  GitBranch,
  FolderGit2,
  Terminal,
} from "lucide-react";
import { memo, useContext, useState } from "react";
import {
  DELEGATION_PHASE_LABEL,
  delegationPhaseSummary,
  delegationRow,
  isLivePhase,
  childThreadIdOf,
  type DelegationPhase,
} from "@/lib/agent-chat/delegation";
import { delegationElapsedLabel, statusTone } from "@/lib/agent-chat/subagents";
import { cn } from "@/lib/utils";
import { openDelegatedChat } from "./delegation-actions";
import { ProviderLogo } from "./provider-logo";
import { TickingText } from "./TickingText";
import type { DelegationEntry } from "./transcript-slots";
import { DurableTasksContext } from "./durable-task-context";
import { plainTaskDetail } from "./SubagentNotificationRow";
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "@/components/ui/hover-card";

const DOT: Record<DelegationPhase, string> = {
  starting: "bg-status-working",
  working: "bg-status-working",
  waiting: "bg-status-working",
  paused: "bg-status-working/60",
  done: "bg-status-open",
  failed: "bg-destructive",
  stopped: "bg-muted-foreground/60",
};

export const DelegationCard = memo(function DelegationCard({
  entries,
}: {
  entries: DelegationEntry[];
}) {
  const [open, setOpen] = useState(false);
  const { tasks } = useContext(DurableTasksContext);
  const rows = entries.map((entry) => {
    const childThread = entry.view ? childThreadIdOf(entry.view.id) : null;
    const task = tasks.find(
      (task) =>
        task.task_id === entry.view?.taskId ||
        (childThread != null && task.child_thread_id === childThread),
    );
    const view =
      entry.view && task
        ? {
            ...entry.view,
            startedAt:
              task.status === "interrupted" && task.completed_at == null
                ? undefined
                : (task.started_at ?? task.created_at),
            finishedAt:
              task.status === "interrupted" && task.completed_at == null
                ? undefined
                : entry.view.finishedAt,
            durationMs:
              task.completed_at == null
                ? undefined
                : task.completed_at - (task.started_at ?? task.created_at),
          }
        : entry.view;
    return {
      entry: { ...entry, view },
      row: delegationRow(entry.call, view),
      task,
    };
  });
  if (!rows.length) return null;
  const live = rows.some(({ row }) => isLivePhase(row.phase));
  return (
    <div data-testid="delegation-card">
      {rows.length > 1 && (
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          className={cn(
            "flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left hover:bg-surface-2 focus-visible:outline-2 focus-visible:outline-ring",
            !live && !open && "opacity-55",
          )}
        >
          <span className="flex -space-x-1.5" aria-hidden>
            {rows.slice(0, 3).map(({ entry, row }) => (
              <span
                key={entry.key}
                className="flex size-6 items-center justify-center rounded-full border border-border/70 bg-muted ring-2 ring-background"
              >
                {row.provider ? (
                  <ProviderLogo provider={row.provider} className="size-3.5" />
                ) : (
                  <Bot className="size-3.5" />
                )}
              </span>
            ))}
            {rows.length > 3 && (
              <span className="flex size-6 items-center justify-center rounded-full bg-muted text-micro">
                +{rows.length - 3}
              </span>
            )}
          </span>
          <span className="min-w-0 flex-1">
            <span className="block text-label font-semibold">
              {rows.length} subagents
            </span>
            <span
              className={cn(
                "block truncate text-micro text-muted-foreground",
                live && statusTone("running").text,
                rows.some(({ row }) => row.phase === "failed") &&
                  "text-destructive",
              )}
            >
              {delegationPhaseSummary(rows.map(({ row }) => row))}
            </span>
          </span>
          <TickingText
            active={live}
            className="text-label tabular-nums text-muted-foreground"
            compute={(now) =>
              delegationElapsedLabel(
                rows.map(({ entry }) => entry.view),
                now,
              )
            }
          />
          <ChevronDown
            className={cn(
              "size-3.5 text-muted-foreground",
              open && "rotate-180",
            )}
          />
        </button>
      )}
      {(rows.length === 1 || open) && (
        <ul
          className={cn(
            "flex flex-col",
            rows.length > 1 &&
              "mt-1 rounded-lg border border-border/60 bg-card/30 p-1",
          )}
        >
          {rows.map(({ entry, row, task }) => {
            const child = row.childThreadId;
            const content = (
              <>
                <span className="relative flex size-6 shrink-0 items-center justify-center rounded-full border border-border/70 bg-muted ring-2 ring-background">
                  {row.provider ? (
                    <ProviderLogo
                      provider={row.provider}
                      className="size-3.5"
                    />
                  ) : (
                    <Bot className="size-3.5" />
                  )}
                  <span
                    className={cn(
                      "absolute -right-px -bottom-px size-2 rounded-full ring-2 ring-background",
                      DOT[row.phase],
                    )}
                  />
                </span>
                <span className="min-w-0 flex-1 text-left">
                  <span className="flex items-baseline gap-2">
                    <span className="truncate text-label font-medium">
                      {row.title}
                    </span>
                    {row.phase !== "done" && (
                      <span
                        data-testid="delegation-row-status"
                        className={cn(
                          "shrink-0 text-micro text-muted-foreground",
                          row.phase === "failed" && "text-destructive",
                        )}
                      >
                        {DELEGATION_PHASE_LABEL[row.phase]}
                      </span>
                    )}
                  </span>
                  <span
                    data-testid="delegation-row-line"
                    className={cn(
                      "block truncate text-caption leading-relaxed text-muted-foreground",
                      row.phase === "failed" && "text-destructive",
                    )}
                  >
                    {plainTaskDetail(row.line)}
                  </span>
                </span>
                <TickingText
                  active={isLivePhase(row.phase)}
                  testId="delegation-row-elapsed"
                  className="shrink-0 text-label tabular-nums text-muted-foreground"
                  compute={(now) => delegationElapsedLabel([entry.view], now)}
                />
                {child && (
                  <ChevronRight className="size-3.5 shrink-0 text-muted-foreground/60 group-hover:text-foreground" />
                )}
              </>
            );
            const className =
              "group flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 hover:bg-surface-2 focus-visible:outline-2 focus-visible:outline-ring";
            const StatusIcon =
              row.phase === "done"
                ? Check
                : row.phase === "failed"
                  ? CircleX
                  : CircleDashed;
            const preview = plainTaskDetail(
              task?.result?.text ?? row.line,
            ).slice(0, 280);
            return (
              <li
                key={entry.key}
                data-testid="delegation-row"
                data-phase={row.phase}
              >
                <HoverCard openDelay={200}>
                  <HoverCardTrigger asChild>
                    {child ? (
                      <button
                        type="button"
                        className={className}
                        aria-label={`Open ${row.title}`}
                        onClick={() => void openDelegatedChat(child)}
                      >
                        {content}
                      </button>
                    ) : (
                      <div tabIndex={0} className={className}>
                        {content}
                      </div>
                    )}
                  </HoverCardTrigger>
                  <HoverCardContent
                    align="start"
                    entrance="subtle"
                    className="w-80 space-y-2 text-caption"
                  >
                    <div className="font-medium">{row.title}</div>
                    <div className="text-muted-foreground">
                      {[row.model, row.effort].filter(Boolean).join(" · ") ||
                        row.providerLabel}
                    </div>
                    <div className="flex items-center gap-1.5 text-muted-foreground">
                      <StatusIcon className="size-3.5" />
                      {DELEGATION_PHASE_LABEL[row.phase]}
                      <TickingText
                        active={isLivePhase(row.phase)}
                        className="ml-auto tabular-nums"
                        compute={(now) =>
                          delegationElapsedLabel([entry.view], now)
                        }
                      />
                    </div>
                    {task?.isolation === "worktree" && (
                      <div className="space-y-1 text-muted-foreground">
                        {task.worktree?.branch && (
                          <div className="flex items-center gap-1.5">
                            <GitBranch className="size-3.5 shrink-0" />
                            <span className="truncate">
                              {task.worktree.branch}
                            </span>
                          </div>
                        )}
                        {(task.worktree?.path || task.cwd) && (
                          <div className="flex items-center gap-1.5">
                            <FolderGit2 className="size-3.5 shrink-0" />
                            <span className="truncate">
                              {task.worktree?.path || task.cwd}
                            </span>
                          </div>
                        )}
                      </div>
                    )}
                    {preview && (
                      <div className="flex gap-1.5 border-t border-border/60 pt-2 text-muted-foreground">
                        <Terminal className="mt-0.5 size-3.5 shrink-0" />
                        <p className="break-words">{preview}</p>
                      </div>
                    )}
                  </HoverCardContent>
                </HoverCard>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
});
