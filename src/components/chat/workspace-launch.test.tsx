/// <reference types="@testing-library/jest-dom/vitest" />
import { cleanup, render, screen } from "@testing-library/react";
import { GitBranch } from "lucide-react";
import { afterEach, describe, expect, it } from "vitest";

import type { ChatViewItem, ToolCallItem } from "@/lib/agent-chat/types";
import { isWorkspaceLaunchCall } from "@/lib/agent-chat/workspace-launch";
import { ToolCallCard } from "./ToolCallCard";
import { describeToolCall } from "./ToolCallStatus";
import { toolCategory, toolIcon } from "./tool-visuals";
import { buildTranscriptSlots } from "./transcript-slots";

afterEach(cleanup);

function launch(overrides: Partial<ToolCallItem> = {}): ToolCallItem {
  return {
    kind: "tool_call", id: "launch-1", seq: 1, turn_id: "t1",
    tool_use_id: "launch-1", tool_name: "mcp__codemux__workspace_launch",
    input: { title: "Fix flaky sidebar test", message: "A self-contained bug fix brief." },
    status: "done", approval_request_id: null,
    result_content: [{ type: "text", text: JSON.stringify({ status: "starting", branch: "fix-sidebar-2" }) }],
    ...overrides,
  };
}

describe("workspace launch visuals", () => {
  it("shows the title and accepted branch with a workspace icon", () => {
    const item = launch();
    render(<ToolCallCard item={item} approval={null} onDecide={() => {}} />);
    expect(screen.getByText("Started workspace")).toBeVisible();
    expect(screen.getByText("Fix flaky sidebar test")).toBeVisible();
    expect(screen.getByText("fix-sidebar-2")).toBeVisible();
    expect(toolIcon(item.tool_name)).toBe(GitBranch);
    expect(toolCategory(item.tool_name)).toBe("agent");
  });

  it("reads generated branches from Codex and MCP result envelopes", () => {
    const text = JSON.stringify({ status: "starting", branch: "fix-sidebar-3" });
    for (const result_content of [text, { content: [{ type: "text", text }] }, { success: true, contentItems: [{ type: "inputText", text }] }]) {
      expect(describeToolCall(launch({ result_content })).argument).toBe("fix-sidebar-3");
    }
  });

  it("handles Codex dynamic calls and launch refusals", () => {
    const item = launch({
      tool_name: "dynamicToolCall",
      input: { tool: "codemux_mcp__codemux__workspace_launch", arguments: JSON.stringify({ title: "Fix terminal resize", message: "Brief", branch: "fix-resize" }) },
      status: "error", result_content: "Branch already exists",
    });
    expect(isWorkspaceLaunchCall(item)).toBe(true);
    expect(describeToolCall(item)).toEqual({ verb: "Workspace launch failed", target: "Fix terminal resize", targetMono: false, argument: "fix-resize" });
    expect(describeToolCall(launch({ status: "running" })).verb).toBe("Starting workspace");
    expect(isWorkspaceLaunchCall(launch({ tool_name: "Bash" }))).toBe(false);
  });

  it.each([false, true])("shows a Codex refusal as an expanded error (streamed: %s)", (streamed) => {
    const item = launch({
      tool_name: "dynamicToolCall",
      input: { tool: "codemux_mcp__codemux__workspace_launch", arguments: JSON.stringify({ title: "Fix terminal resize", message: "Brief", branch: "fix-resize" }) },
      status: "done",
      result_content: { success: false, contentItems: [{ type: "inputText", text: "Branch 'fix-resize' already exists or is starting. Choose a new branch." }] },
    });
    const { container, rerender } = render(<ToolCallCard item={streamed ? { ...item, status: "running", result_content: null } : item} approval={null} onDecide={() => {}} />);
    if (streamed) {
      expect(screen.getByRole("button", { name: /Starting workspace/ })).toHaveAttribute("aria-expanded", "false");
      rerender(<ToolCallCard item={item} approval={null} onDecide={() => {}} />);
    }
    expect(screen.getByRole("button", { name: /Workspace launch failed Fix terminal resize fix-resize/ })).toHaveAttribute("aria-expanded", "true");
    expect(screen.queryByText("Started workspace")).not.toBeInTheDocument();
    expect(container.querySelector(".lucide-x")).toBeInTheDocument();
    expect(container.querySelector(".lucide-check")).not.toBeInTheDocument();
    expect(screen.getByText(/Branch 'fix-resize' already exists or is starting/)).toBeVisible();
  });

  it("keeps launches visible beside the reply after the turn settles", () => {
    const item = launch();
    const messages: ChatViewItem[] = [
      { kind: "user_message", id: "u1", seq: 0, text: "Start separate workspaces", created_at: 0 },
      item,
      { kind: "assistant_message", id: "a1", seq: 2, turn_id: "t1", text: "Started the workspace.", streaming: false },
      { kind: "turn_ended", id: "end", seq: 3, turn_id: "t1", status: { kind: "success" }, completed_at: 2000 },
    ];
    for (const streaming of [true, false]) {
      const slots = buildTranscriptSlots(messages, streaming);
      expect(slots.some(({ body }) => body.kind === "item" && body.item === item)).toBe(true);
      expect(slots.some(({ body }) => body.kind === "activity" || body.kind === "turn_fold")).toBe(false);
    }
  });
});
