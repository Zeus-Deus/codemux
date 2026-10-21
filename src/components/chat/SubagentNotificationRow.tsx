import { Bot, ChevronRight, Server } from "lucide-react";
import { useContext } from "react";
import type { MessageNotification } from "@/tauri/events";
import { providerKindOf } from "@/lib/agent-chat/delegation";
import { ProviderLogo } from "./provider-logo";
import { cn } from "@/lib/utils";
import { DurableTasksContext } from "./durable-task-context";
import { RemoteTaskContext } from "./remote-task-context";
import { openDelegatedChat } from "./delegation-actions";

const OUTCOME = {
  completed: "Finished",
  failed: "Failed",
  cancelled: "Stopped",
  updated: "Updated",
  unknown: "Needs attention",
};
export const plainTaskDetail = (text: string) =>
  text
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/[`*_#>]/g, "")
    .replace(/\s+/g, " ")
    .trim();

/** Only presentation metadata and the durable result are rendered. Agent
 * framing is deliberately absent from this component's inputs. */
export function SubagentNotificationRow({
  notification,
  timestamp,
}: {
  notification: MessageNotification;
  timestamp?: number;
}) {
  const { tasks } = useContext(DurableTasksContext);
  const remote = useContext(RemoteTaskContext);
  const task = tasks.find((task) =>
    notification.source.task_ids.includes(task.task_id),
  );
  const ssh = notification.source.kind === "remote_task";
  const snapshot = notification.tasks?.length === 1 ? notification.tasks[0] : null;
  const provider = providerKindOf(task?.provider ?? snapshot?.provider);
  const child = task?.child_thread_id ?? notification.source.child_thread_id;
  const remoteId =
    task?.remote_task_id ??
    notification.source.task_ids[0]?.replace(/^ssh:/, "");
  const open = ssh
    ? remote && remoteId
      ? () => remote.onOpen(remoteId)
      : null
    : child
      ? () => void openDelegatedChat(child)
      : null;
  const title = task?.title ?? snapshot?.title ?? notification.summary;
  const detail = ssh
    ? task?.host?.name
      ? `${task.host.name} · ${OUTCOME[notification.outcome]}`
      : ""
    : plainTaskDetail(task?.result?.text ?? "");
  const content = (
    <>
      <span className="flex size-6 shrink-0 items-center justify-center rounded-full border border-border/70 bg-muted ring-2 ring-background">
        {ssh ? (
          <Server className="size-3.5" />
        ) : provider ? (
          <ProviderLogo provider={provider} className="size-3.5" />
        ) : (
          <Bot className="size-3.5" />
        )}
      </span>
      <span className="min-w-0 flex-1 text-left">
        <span className="flex items-baseline gap-2">
          <span className="truncate text-label font-medium">{title}</span>
          <span
            className={cn(
              "text-micro text-muted-foreground",
              notification.outcome === "failed" && "text-destructive",
            )}
          >
            {OUTCOME[notification.outcome]}
          </span>
        </span>
        {detail && (
          <span className="block truncate text-caption text-muted-foreground">
            {detail}
          </span>
        )}
      </span>
      {timestamp != null && (
        <time
          className="shrink-0 text-caption text-muted-foreground"
          dateTime={new Date(timestamp).toISOString()}
        >
          {new Date(timestamp).toLocaleTimeString([], {
            hour: "2-digit",
            minute: "2-digit",
          })}
        </time>
      )}
      {open && (
        <ChevronRight className="size-3.5 shrink-0 text-muted-foreground/60" />
      )}
    </>
  );
  const className =
    "flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 hover:bg-surface-2 focus-visible:outline-2 focus-visible:outline-ring";
  return open ? (
    <button
      type="button"
      data-testid="subagent-notification-row"
      aria-label={`Open ${ssh ? "remote task" : "subagent"} ${title}`}
      className={className}
      onClick={open}
    >
      {content}
    </button>
  ) : (
    <div data-testid="subagent-notification-row" className={className}>
      {content}
    </div>
  );
}
