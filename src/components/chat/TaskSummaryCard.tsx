import { ChevronRight, ListTodo } from "lucide-react";
import { memo, useCallback } from "react";

import { cn } from "@/lib/utils";
import { useUIStore } from "@/stores/ui-store";
import type { ToolCallItem } from "@/lib/agent-chat/types";

/**
 * Task-list receipt line. Rendered for `TodoWrite`-style tool calls — a
 * tool whose input carries a `todos` array. The full list used to be
 * printed here, duplicating the Tasks panel with a copy that never
 * updated; now the thread just records that a plan landed ("Task list
 * created · 3 items") and the panel stays the single live truth.
 * Clicking the receipt flips the right panel to the Tasks tab when a
 * `workspaceId` is available (same pattern as `WorkflowRunCard`'s
 * "Open panel"); absent → the row is inert rather than throwing. We
 * never fabricate a receipt from prose: no `todos` array → this
 * component is not used (see `extractTodos` / the MessageList dispatch).
 */
export const TaskSummaryCard = memo(function TaskSummaryCard({
  item,
  workspaceId = null,
}: {
  item: ToolCallItem;
  workspaceId?: string | null;
}) {
  const openPanel = useCallback(() => {
    if (workspaceId) useUIStore.getState().setRightPanelTab(workspaceId, "tasks");
  }, [workspaceId]);

  const todos = extractTodos(item.input);
  if (todos.length === 0) return null;

  const done = todos.filter((t) => t.status === "completed").length;
  const interactive = workspaceId != null;

  return (
    <button
      type="button"
      onClick={openPanel}
      disabled={!interactive}
      data-testid="task-receipt"
      className={cn(
        "flex w-full items-center gap-2.5 rounded-[11px] border border-border/60 bg-foreground/[0.025] px-3 py-2 text-left transition-colors",
        interactive
          ? "cursor-pointer hover:border-border hover:bg-foreground/[0.05]"
          : "cursor-default",
      )}
    >
      <ListTodo
        className="size-[15px] shrink-0 text-muted-foreground"
        strokeWidth={1.6}
        aria-hidden
      />
      <span className="min-w-0 flex-1 truncate text-xs text-foreground/80">
        {done > 0 ? "Task list updated" : "Task list created"} ·{" "}
        <span className="font-semibold text-foreground">
          {done > 0
            ? `${done}/${todos.length} done`
            : `${todos.length} item${todos.length === 1 ? "" : "s"}`}
        </span>
      </span>
      {interactive && (
        <>
          <span className="shrink-0 font-mono text-[10.5px] text-muted-foreground">
            Open in panel
          </span>
          <ChevronRight
            className="size-3 shrink-0 text-muted-foreground"
            strokeWidth={1.7}
            aria-hidden
          />
        </>
      )}
    </button>
  );
});

export interface TodoEntry {
  label: string;
  status: "pending" | "in_progress" | "completed";
}

/** Pull a normalized todo list out of a `TodoWrite` tool input. Accepts
 *  either `content` or `activeForm` for the label text and tolerates
 *  unknown status strings (treated as pending). Returns `[]` when the
 *  input carries no usable `todos` array. */
export function extractTodos(input: unknown): TodoEntry[] {
  if (!isRecord(input)) return [];
  const raw = input.todos;
  if (!Array.isArray(raw)) return [];
  const out: TodoEntry[] = [];
  for (const entry of raw) {
    if (!isRecord(entry)) continue;
    const label =
      stringField(entry, "content") ?? stringField(entry, "activeForm");
    if (label == null) continue;
    const status = entry.status;
    out.push({
      label,
      status:
        status === "completed" || status === "in_progress"
          ? status
          : "pending",
    });
  }
  return out;
}

/** Whether a tool call should render as a task summary rather than a
 *  generic tool card. */
export function isTaskSummaryTool(item: ToolCallItem): boolean {
  return item.tool_name === "TodoWrite" && extractTodos(item.input).length > 0;
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function stringField(obj: Record<string, unknown>, key: string): string | null {
  const v = obj[key];
  return typeof v === "string" && v.length > 0 ? v : null;
}
