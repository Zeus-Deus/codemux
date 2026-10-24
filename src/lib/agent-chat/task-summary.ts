import type { TasksSnapshot } from "@/tauri/events";

/** Counts + run state behind the composer strip's Tasks row. */
export interface TaskChipSummary {
  completed: number;
  total: number;
  /** Whether the row should present as LIVE (spinner). */
  running: boolean;
  /** The step the plan is on: the one in progress, else the next one
   *  pending, else (all done) the last one. */
  current: string;
}

/**
 * Derive the composer Tasks chip's state from a thread's provider plan.
 *
 * The `TasksUpdated` snapshot is **durable state**: it is persisted and
 * hydrate-replayed, and nothing ever rewrites it when a run ends. So an
 * `in_progress` row is not evidence that work is happening — only the
 * thread's own `streaming` flag is. Gating the spinner on `streaming`
 * keeps a plan the provider left mid-step from spinning forever (including
 * across a restart) while still showing the counts: the durable snapshot
 * is by design, so the chip stays, it just stops claiming to be live.
 */
export function taskChipSummary(
  tasks: TasksSnapshot | null | undefined,
  streaming: boolean,
): TaskChipSummary | null {
  if (!tasks || tasks.tasks.length === 0) return null;
  const list = tasks.tasks;
  const current =
    list.find((task) => task.status === "in_progress") ??
    list.find((task) => task.status === "pending") ??
    list[list.length - 1];
  return {
    completed: list.filter((task) => task.status === "completed").length,
    total: list.length,
    running: streaming && list.some((task) => task.status === "in_progress"),
    current: current.title,
  };
}
