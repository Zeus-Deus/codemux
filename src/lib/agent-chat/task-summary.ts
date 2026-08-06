import type { TasksSnapshot } from "@/tauri/events";

/** Counts + run state behind the composer's `Tasks N/M` chip. */
export interface TaskChipSummary {
  completed: number;
  total: number;
  /** Whether the chip should present as LIVE (amber + spinner). */
  running: boolean;
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
  return {
    completed: tasks.tasks.filter((task) => task.status === "completed").length,
    total: tasks.tasks.length,
    running:
      streaming && tasks.tasks.some((task) => task.status === "in_progress"),
  };
}
