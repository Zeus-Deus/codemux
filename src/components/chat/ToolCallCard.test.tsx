/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, it, expect, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("@/lib/toast", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    info: vi.fn(),
  },
}));

afterEach(() => cleanup());

import type {
  PermissionRequestItem,
  ToolCallItem,
} from "@/lib/agent-chat/types";
import { toast } from "@/lib/toast";

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
  beforeEach(() => {
    vi.mocked(toast.success).mockReset();
  });

  it("pending_approval state renders the approval controls and input preview", () => {
    const onDecide = vi.fn();
    render(
      <ToolCallCard
        item={makeTool({ approval_request_id: "req-1" })}
        approval={makePendingApproval()}
        onDecide={onDecide}
      />,
    );
    expect(screen.getByText("Allow")).toBeInTheDocument();
    // The persistent variants live behind the "Allow always" dropdown
    // trigger since Stage 5 — the flat session-only button is gone.
    expect(screen.getByText("Allow always")).toBeInTheDocument();
    expect(screen.getByText("Deny")).toBeInTheDocument();
  });

  it("flat 'Allow' dispatches a one-shot allow with NO updated_permissions", () => {
    const onDecide = vi.fn();
    render(
      <ToolCallCard
        item={makeTool({ approval_request_id: "req-1" })}
        approval={makePendingApproval()}
        onDecide={onDecide}
      />,
    );
    fireEvent.click(screen.getByText("Allow"));
    // Stage 5 contract: one-shot allows omit the field entirely so the
    // SDK doesn't persist any rule.
    expect(onDecide).toHaveBeenCalledWith({ decision: "allow" });
    const payload = onDecide.mock.calls[0][0];
    expect(payload).not.toHaveProperty("updated_permissions");
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("'Allow always' dropdown shows two persistent scopes", async () => {
    const user = userEvent.setup();
    render(
      <ToolCallCard
        item={makeTool({ approval_request_id: "req-1" })}
        approval={makePendingApproval()}
        onDecide={vi.fn()}
      />,
    );
    await user.click(screen.getByText("Allow always"));
    expect(
      await screen.findByRole("menuitem", { name: /For this project/ }),
    ).toBeInTheDocument();
    expect(
      await screen.findByRole("menuitem", { name: /For all projects/ }),
    ).toBeInTheDocument();
  });

  it("'For this project' adds an addRules entry to localSettings + fires a project toast", async () => {
    const user = userEvent.setup();
    const onDecide = vi.fn();
    render(
      <ToolCallCard
        item={makeTool({ approval_request_id: "req-1" })}
        approval={makePendingApproval()}
        onDecide={onDecide}
      />,
    );
    await user.click(screen.getByText("Allow always"));
    await user.click(
      await screen.findByRole("menuitem", { name: /For this project/ }),
    );
    expect(onDecide).toHaveBeenCalledWith({
      decision: "allow",
      updated_permissions: [
        {
          type: "addRules",
          rules: [{ toolName: "Bash" }],
          behavior: "allow",
          destination: "localSettings",
        },
      ],
    });
    expect(toast.success).toHaveBeenCalledWith(
      "Allowing Bash for this project",
      { description: "Rule saved to .claude/settings.local.json" },
    );
  });

  it("'For all projects' adds an addRules entry to userSettings + fires a user toast", async () => {
    const user = userEvent.setup();
    const onDecide = vi.fn();
    render(
      <ToolCallCard
        item={makeTool({ approval_request_id: "req-1", tool_name: "Read" })}
        approval={makePendingApproval({
          payload: { tool_name: "Read", tool_input: { path: "/etc/hosts" } },
        })}
        onDecide={onDecide}
      />,
    );
    await user.click(screen.getByText("Allow always"));
    await user.click(
      await screen.findByRole("menuitem", { name: /For all projects/ }),
    );
    expect(onDecide).toHaveBeenCalledWith({
      decision: "allow",
      updated_permissions: [
        {
          type: "addRules",
          rules: [{ toolName: "Read" }],
          behavior: "allow",
          destination: "userSettings",
        },
      ],
    });
    expect(toast.success).toHaveBeenCalledWith(
      "Allowing Read for all projects",
      { description: "Rule saved to ~/.claude/settings.json" },
    );
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
    expect(container.textContent).not.toContain("Allow always");
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

  // ─────────────────────────────────────────────────────────────────
  // Stage 5 hardening — double-submit guard, request_id remount,
  // soft toast wording. Edge cases identified by the Stage 5 review.
  // ─────────────────────────────────────────────────────────────────

  it("rapid double-click on Allow dispatches onDecide exactly once", () => {
    const onDecide = vi.fn();
    render(
      <ToolCallCard
        item={makeTool({ approval_request_id: "req-1" })}
        approval={makePendingApproval()}
        onDecide={onDecide}
      />,
    );
    const allow = screen.getByText("Allow");
    // Two clicks fired before React has a chance to re-render with
    // approval.resolution = "responding" — without the in-flight ref
    // the SDK would receive two decisions for the same request_id.
    fireEvent.click(allow);
    fireEvent.click(allow);
    expect(onDecide).toHaveBeenCalledTimes(1);
  });

  it("rapid double-pick on a dropdown item dispatches onDecide exactly once", async () => {
    const user = userEvent.setup();
    const onDecide = vi.fn();
    render(
      <ToolCallCard
        item={makeTool({ approval_request_id: "req-1" })}
        approval={makePendingApproval()}
        onDecide={onDecide}
      />,
    );
    await user.click(screen.getByText("Allow always"));
    const item = await screen.findByRole("menuitem", {
      name: /For this project/,
    });
    // Two onSelect-equivalent fires; only the first should reach
    // the SDK. Toast also fires only once.
    fireEvent.click(item);
    fireEvent.click(item);
    expect(onDecide).toHaveBeenCalledTimes(1);
    expect(toast.success).toHaveBeenCalledTimes(1);
  });

  it("rapid double-click on Confirm deny dispatches onDecide exactly once", () => {
    const onDecide = vi.fn();
    render(
      <ToolCallCard
        item={makeTool({ approval_request_id: "req-1" })}
        approval={makePendingApproval()}
        onDecide={onDecide}
      />,
    );
    fireEvent.click(screen.getByText("Deny"));
    fireEvent.change(screen.getByPlaceholderText("Reason (optional)"), {
      target: { value: "no" },
    });
    const confirm = screen.getByText("Confirm deny");
    fireEvent.click(confirm);
    fireEvent.click(confirm);
    expect(onDecide).toHaveBeenCalledTimes(1);
  });

  it("changing approval.request_id remounts the footer and clears the deny textarea", () => {
    const onDecide = vi.fn();
    const { rerender } = render(
      <ToolCallCard
        item={makeTool({ approval_request_id: "req-1" })}
        approval={makePendingApproval({ request_id: "req-1" })}
        onDecide={onDecide}
      />,
    );
    // Open deny on approval #1 and type a reason.
    fireEvent.click(screen.getByText("Deny"));
    fireEvent.change(screen.getByPlaceholderText("Reason (optional)"), {
      target: { value: "leftover text from approval 1" },
    });
    // A new approval lands (different request_id) on the same
    // ToolCallCard instance. The footer must remount fresh — the
    // user should not see stale text from a prior denial.
    rerender(
      <ToolCallCard
        item={makeTool({ approval_request_id: "req-2" })}
        approval={makePendingApproval({ request_id: "req-2" })}
        onDecide={onDecide}
      />,
    );
    // Back to the action row (Allow + Allow always + Deny), no
    // textarea showing.
    expect(screen.getByText("Allow")).toBeInTheDocument();
    expect(screen.queryByPlaceholderText("Reason (optional)")).toBeNull();
  });

  it("toast does NOT fire for the one-shot 'Allow' (only persistent scopes show toasts)", () => {
    const onDecide = vi.fn();
    render(
      <ToolCallCard
        item={makeTool({ approval_request_id: "req-1" })}
        approval={makePendingApproval()}
        onDecide={onDecide}
      />,
    );
    fireEvent.click(screen.getByText("Allow"));
    expect(toast.success).not.toHaveBeenCalled();
  });
});
