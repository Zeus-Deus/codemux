/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import type { ToolCallItem } from "@/lib/agent-chat/types";

import { ToolGroupCard, deriveGroupTitle } from "./ToolGroupCard";

afterEach(() => cleanup());

function tool(seq: number, tool_name: string, input: unknown): ToolCallItem {
  return {
    kind: "tool_call",
    id: `tc-${seq}`,
    seq,
    tool_use_id: `tu-${seq}`,
    tool_name,
    input,
    status: "done",
    result_content: null,
    approval_request_id: null,
  };
}

describe("deriveGroupTitle", () => {
  it("labels an all search/read run as 'Searched the codebase'", () => {
    expect(
      deriveGroupTitle([
        tool(0, "Grep", { pattern: "x" }),
        tool(1, "Read", { file_path: "/a" }),
      ]),
    ).toBe("Searched the codebase");
  });

  it("labels an all-Bash run as 'Ran commands'", () => {
    expect(
      deriveGroupTitle([
        tool(0, "Bash", { command: "ls" }),
        tool(1, "Bash", { command: "pwd" }),
      ]),
    ).toBe("Ran commands");
  });

  it("labels a mixed run as 'Ran N tools'", () => {
    expect(
      deriveGroupTitle([
        tool(0, "Bash", { command: "ls" }),
        tool(1, "Read", { file_path: "/a" }),
        tool(2, "SomethingElse", {}),
      ]),
    ).toBe("Ran 3 tools");
  });
});

describe("ToolGroupCard", () => {
  it("shows the title + command count and reveals per-call rows on expand", () => {
    render(
      <ToolGroupCard
        items={[
          tool(0, "Grep", { pattern: "__cause__", path: "src" }),
          tool(1, "Read", { file_path: "gateway/run.py" }),
        ]}
      />,
    );
    expect(screen.getByText("Searched the codebase")).toBeInTheDocument();
    expect(screen.getByText("2 commands")).toBeInTheDocument();
    // Body collapsed by default.
    expect(screen.queryByText("gateway/run.py")).toBeNull();

    fireEvent.click(screen.getByText("Searched the codebase"));
    // Per-call rows: lowercased verb + one-line input.
    expect(screen.getByText("grep")).toBeInTheDocument();
    expect(screen.getByText("read")).toBeInTheDocument();
    expect(screen.getByText("gateway/run.py")).toBeInTheDocument();
  });
});
