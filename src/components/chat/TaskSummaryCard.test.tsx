/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

import type { ToolCallItem } from "@/lib/agent-chat/types";

import {
  TaskSummaryCard,
  extractTodos,
  isTaskSummaryTool,
} from "./TaskSummaryCard";

afterEach(() => cleanup());

function todoTool(input: unknown): ToolCallItem {
  return {
    kind: "tool_call",
    id: "tc-todo",
    seq: 0,
    tool_use_id: "tu-todo",
    tool_name: "TodoWrite",
    input,
    status: "done",
    result_content: null,
    approval_request_id: null,
  };
}

describe("extractTodos", () => {
  it("reads content or activeForm labels and normalizes status", () => {
    const todos = extractTodos({
      todos: [
        { content: "Wire the reducer", status: "completed" },
        { activeForm: "Rendering the card", status: "in_progress" },
        { content: "Write tests", status: "pending" },
        { content: "Unknown status", status: "weird" },
      ],
    });
    expect(todos).toEqual([
      { label: "Wire the reducer", status: "completed" },
      { label: "Rendering the card", status: "in_progress" },
      { label: "Write tests", status: "pending" },
      { label: "Unknown status", status: "pending" },
    ]);
  });

  it("returns [] when there is no usable todos array", () => {
    expect(extractTodos({ notTodos: 1 })).toEqual([]);
    expect(extractTodos(null)).toEqual([]);
    expect(extractTodos({ todos: "nope" })).toEqual([]);
  });
});

describe("isTaskSummaryTool", () => {
  it("is true only for a TodoWrite carrying a todos array", () => {
    expect(isTaskSummaryTool(todoTool({ todos: [{ content: "a", status: "pending" }] }))).toBe(true);
    expect(isTaskSummaryTool(todoTool({}))).toBe(false);
    expect(
      isTaskSummaryTool({ ...todoTool({ todos: [] }), tool_name: "Read" }),
    ).toBe(false);
  });
});

describe("TaskSummaryCard", () => {
  it("renders every todo label and a done/total summary", () => {
    render(
      <TaskSummaryCard
        item={todoTool({
          todos: [
            { content: "Branch off origin/main", status: "completed" },
            { content: "Add regression test", status: "completed" },
            { content: "Open the PR", status: "pending" },
          ],
        })}
      />,
    );
    expect(screen.getByText("Task list · 2/3 done")).toBeInTheDocument();
    expect(screen.getByText("Branch off origin/main")).toBeInTheDocument();
    expect(screen.getByText("Add regression test")).toBeInTheDocument();
    expect(screen.getByText("Open the PR")).toBeInTheDocument();
  });

  it("renders nothing when there are no todos", () => {
    const { container } = render(<TaskSummaryCard item={todoTool({})} />);
    expect(container.firstChild).toBeNull();
  });
});
