/// <reference types="@testing-library/jest-dom/vitest" />
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { TasksPanel, tasksToMarkdown } from "./TasksPanel";
import type { TasksSnapshot } from "@/tauri/events";

afterEach(cleanup);

const midRunSnapshot: TasksSnapshot = {
  explanation: "Implement the task surface end-to-end.",
  tasks: [
    {
      task_id: "research",
      title: "Research providers",
      status: "completed",
      blocked_by: [],
    },
    {
      task_id: "build",
      title: "Build the panel",
      status: "in_progress",
      blocked_by: [],
    },
    {
      task_id: "verify",
      title: "Verify visually",
      status: "pending",
      blocked_by: ["build"],
    },
  ],
};

describe("TasksPanel", () => {
  it("renders status, progress, provider explanation, and blockers", () => {
    render(<TasksPanel snapshot={midRunSnapshot} />);

    expect(screen.getByTestId("tasks-status-badge")).toHaveTextContent(
      "Working",
    );
    expect(screen.getByText("1/3 done")).toBeInTheDocument();
    expect(
      screen.getByText("Implement the task surface end-to-end."),
    ).toBeInTheDocument();
    expect(screen.getByRole("progressbar")).toHaveAttribute(
      "aria-valuenow",
      "33",
    );
    expect(screen.getByText("Blocked by Build the panel")).toBeInTheDocument();
    expect(screen.getByText("Research providers")).toHaveClass("line-through");
    // Rows are numbered so "step 2" in the thread can be matched up.
    expect(screen.getByText("2")).toBeInTheDocument();
  });

  it("reports Complete once every row is done, Queued before any starts", () => {
    const done: TasksSnapshot = {
      tasks: midRunSnapshot.tasks.map((task) => ({
        ...task,
        status: "completed",
      })),
    };
    const { rerender } = render(<TasksPanel snapshot={done} />);
    expect(screen.getByTestId("tasks-status-badge")).toHaveTextContent(
      "Complete",
    );

    const queued: TasksSnapshot = {
      tasks: midRunSnapshot.tasks.map((task) => ({
        ...task,
        status: "pending",
      })),
    };
    rerender(<TasksPanel snapshot={queued} />);
    expect(screen.getByTestId("tasks-status-badge")).toHaveTextContent(
      "Queued",
    );
  });

  it("shows the last-update time when provided", () => {
    render(
      <TasksPanel
        snapshot={midRunSnapshot}
        updatedAt={new Date(2026, 0, 1, 16, 45, 39).getTime()}
      />,
    );
    expect(screen.getByText(/45:39/)).toBeInTheDocument();
  });
});

describe("tasksToMarkdown", () => {
  it("emits a checklist with the explanation as preamble", () => {
    expect(tasksToMarkdown(midRunSnapshot)).toBe(
      [
        "Implement the task surface end-to-end.",
        "",
        "- [x] Research providers",
        "- [ ] Build the panel",
        "- [ ] Verify visually",
      ].join("\n"),
    );
  });
});
