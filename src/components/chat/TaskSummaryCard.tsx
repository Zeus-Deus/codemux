import { Circle, CircleCheck } from "lucide-react";
import { memo } from "react";

import { cn } from "@/lib/utils";
import type { ToolCallItem } from "@/lib/agent-chat/types";

/**
 * Task-list summary card (design D8). Rendered for `TodoWrite`-style tool
 * calls — a tool whose input carries a `todos` array. Completed items get
 * a green circled check; everything else a hollow circle. We never
 * fabricate a summary from prose: no `todos` array → this component is
 * not used (see `extractTodos` / the MessageList dispatch).
 */
export const TaskSummaryCard = memo(function TaskSummaryCard({
  item,
}: {
  item: ToolCallItem;
}) {
  const todos = extractTodos(item.input);
  if (todos.length === 0) return null;

  const done = todos.filter((t) => t.status === "completed").length;

  return (
    <div className="rounded-[11px] border border-border/60 bg-foreground/[0.025] px-[15px] py-[14px]">
      <div className="mb-2.5 text-[13px] font-bold text-foreground">
        Task list · {done}/{todos.length} done
      </div>
      <div className="flex flex-col gap-2">
        {todos.map((todo, i) => {
          const complete = todo.status === "completed";
          return (
            <div
              key={i}
              className="flex items-start gap-[9px] text-[12.5px] leading-[1.5] text-foreground"
            >
              {complete ? (
                <CircleCheck
                  className="mt-px h-[15px] w-[15px] shrink-0 text-status-open"
                  strokeWidth={1.7}
                  aria-hidden
                />
              ) : (
                <Circle
                  className="mt-px h-[15px] w-[15px] shrink-0 text-muted-foreground/40"
                  strokeWidth={1.7}
                  aria-hidden
                />
              )}
              <span className={cn(complete && "text-muted-foreground")}>
                {todo.label}
              </span>
            </div>
          );
        })}
      </div>
    </div>
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
