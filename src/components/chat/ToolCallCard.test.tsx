/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

afterEach(() => cleanup());

import type {
  PermissionRequestItem,
  ToolCallItem,
} from "@/lib/agent-chat/types";

import { ToolCallCard } from "./ToolCallCard";

function makeTool(overrides: Partial<ToolCallItem> = {}): ToolCallItem {
  return {
    kind: "tool_call",
    id: "tool-1",
    seq: 0,
    tool_use_id: "tu-1",
    tool_name: "Bash",
    input: { command: "ls -la" },
    status: "running",
    result_content: null,
    approval_request_id: null,
    ...overrides,
  };
}

function makePendingApproval(
  overrides: Partial<PermissionRequestItem> = {},
): PermissionRequestItem {
  return {
    kind: "permission_request",
    id: "req-1",
    seq: 1,
    request_id: "req-1",
    turn_id: "turn-1",
    request_kind: "command",
    payload: { tool_name: "Bash", tool_input: { command: "ls -la" } },
    tool_use_id: "tu-1",
    resolution: { state: "pending" },
    ...overrides,
  };
}

describe("ToolCallCard", () => {
  it("pending_approval state renders the approval buttons and input preview", () => {
    const onDecide = vi.fn();
    render(
      <ToolCallCard
        item={makeTool({ approval_request_id: "req-1" })}
        approval={makePendingApproval()}
        onDecide={onDecide}
      />,
    );
    expect(screen.getByText("Allow")).toBeInTheDocument();
    expect(screen.getByText("Allow for session")).toBeInTheDocument();
    expect(screen.getByText("Deny")).toBeInTheDocument();

    fireEvent.click(screen.getByText("Allow"));
    expect(onDecide).toHaveBeenCalledWith({ decision: "allow" });
  });

  it("Allow for session dispatches the allow_for_session decision", () => {
    const onDecide = vi.fn();
    render(
      <ToolCallCard
        item={makeTool({ approval_request_id: "req-1" })}
        approval={makePendingApproval()}
        onDecide={onDecide}
      />,
    );
    fireEvent.click(screen.getByText("Allow for session"));
    expect(onDecide).toHaveBeenCalledWith({ decision: "allow_for_session" });
  });

  it("Deny reveals the reason textarea and Confirm deny ships the reason", () => {
    const onDecide = vi.fn();
    render(
      <ToolCallCard
        item={makeTool({ approval_request_id: "req-1" })}
        approval={makePendingApproval()}
        onDecide={onDecide}
      />,
    );
    fireEvent.click(screen.getByText("Deny"));
    const textarea = screen.getByPlaceholderText("Reason (optional)");
    fireEvent.change(textarea, { target: { value: "looks dangerous" } });
    fireEvent.click(screen.getByText("Confirm deny"));
    expect(onDecide).toHaveBeenCalledWith({
      decision: "deny",
      message: "looks dangerous",
    });
  });

  it("responding state renders Submitting decision…", () => {
    const item = makeTool({ approval_request_id: "req-1" });
    const approval = makePendingApproval({
      resolution: { state: "responding", decision: { decision: "allow" } },
    });
    render(<ToolCallCard item={item} approval={approval} onDecide={() => {}} />);
    expect(screen.getByText(/Submitting decision/)).toBeInTheDocument();
  });

  it("denied state renders the denial label and suppresses the body", () => {
    const item = makeTool({
      approval_request_id: "req-1",
      status: "running",
    });
    const approval = makePendingApproval({
      resolution: {
        state: "resolved",
        decision: { decision: "deny", message: "nope" },
      },
    });
    const { container } = render(
      <ToolCallCard item={item} approval={approval} onDecide={() => {}} />,
    );
    expect(screen.getByText(/Denied: nope/)).toBeInTheDocument();
    // No allow/deny buttons when resolved.
    expect(container.textContent).not.toContain("Allow for session");
  });

  it("executing state shows the tool call header without an approval footer", () => {
    // bypassPermissions path: no approval ever arrives.
    render(
      <ToolCallCard
        item={makeTool()}
        approval={null}
        onDecide={() => {}}
      />,
    );
    expect(screen.queryByText("Allow")).toBeNull();
    // ToolCallStatus renders "Ran" as the verb for Bash.
    expect(screen.getByText("Ran")).toBeInTheDocument();
  });

  it("success state collapses the body by default; expanding reveals the result", () => {
    const item = makeTool({
      status: "done",
      result_content: "line1\nline2",
    });
    const { container } = render(
      <ToolCallCard item={item} approval={null} onDecide={() => {}} />,
    );
    // Body collapsed by default on success.
    expect(container.textContent).not.toContain("line1");

    // Expand via the chevron button.
    const toggle = container.querySelector('button[aria-label="Expand"]');
    expect(toggle).toBeTruthy();
    fireEvent.click(toggle as HTMLElement);
    expect(container.textContent).toContain("line1");
  });

  it("error state auto-expands the body and keeps the target text visible", () => {
    const item = makeTool({
      status: "error",
      result_content: "permission denied",
    });
    render(<ToolCallCard item={item} approval={null} onDecide={() => {}} />);
    // Body visible without user action because error auto-expands.
    expect(screen.getByText(/permission denied/)).toBeInTheDocument();
  });

  it("unknown tool uses the fallback icon and passes through", () => {
    render(
      <ToolCallCard
        item={makeTool({ tool_name: "BrandNewTool" })}
        approval={null}
        onDecide={() => {}}
      />,
    );
    // ToolCallStatus fallback renders "Called <tool_name>".
    expect(screen.getByText(/Called BrandNewTool/)).toBeInTheDocument();
  });
});
