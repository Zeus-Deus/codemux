import { describe, it, expect } from "vitest";

import type { TaskSnapshotItem, TasksSnapshot } from "@/tauri/events";

import { taskChipSummary } from "./task-summary";

function snapshot(...statuses: TaskSnapshotItem["status"][]): TasksSnapshot {
  return {
    tasks: statuses.map((status, index) => ({
      task_id: `t${index}`,
      title: `Task ${index}`,
      status,
      blocked_by: [],
    })),
  };
}

describe("taskChipSummary", () => {
  it("renders no chip without a plan", () => {
    expect(taskChipSummary(null, true)).toBeNull();
    expect(taskChipSummary(undefined, true)).toBeNull();
    expect(taskChipSummary({ tasks: [] }, true)).toBeNull();
  });

  it("reports live progress while the thread streams", () => {
    expect(
      taskChipSummary(snapshot("completed", "in_progress", "pending"), true),
    ).toEqual({ completed: 1, total: 3, running: true });
  });

  it("keeps the counts but drops the spinner once the run is over", () => {
    // The snapshot is durable — nothing rewrites the `in_progress` row when
    // the turn ends, so only `streaming` can settle the chip.
    expect(
      taskChipSummary(snapshot("completed", "in_progress", "pending"), false),
    ).toEqual({ completed: 1, total: 3, running: false });
  });

  it("still reads complete when every row is done", () => {
    expect(taskChipSummary(snapshot("completed", "completed"), false)).toEqual({
      completed: 2,
      total: 2,
      running: false,
    });
  });
});
