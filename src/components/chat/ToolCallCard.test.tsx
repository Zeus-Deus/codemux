/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, it, expect, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
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

    // The whole header row is the toggle, named by the command it ran.
    const toggle = screen.getByRole("button", { name: /Ran\s+ls -la/ });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    // Nothing to point at until the body mounts on first open.
    expect(toggle).not.toHaveAttribute("aria-controls");
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(container.textContent).toContain("line1");
    const body = document.getElementById(
      toggle.getAttribute("aria-controls") ?? "",
    );
    expect(body).toHaveAttribute("data-state", "open");
    expect(body?.className).toContain("motion-reduce:transition-none");

    // Collapsing keeps the body mounted so it can ease shut, but hides it
    // from assistive tech and the tab order.
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(body).toHaveAttribute("data-state", "closed");
    expect(body).toHaveAttribute("aria-hidden", "true");
    expect(body).toHaveAttribute("inert");
  });

  it("keeps the header a plain row while an approval is pending", () => {
    render(
      <ToolCallCard
        item={makeTool({ approval_request_id: "req-1" })}
        approval={makePendingApproval()}
        onDecide={() => {}}
      />,
    );
    expect(screen.queryByRole("button", { name: /Ran\s+ls -la/ })).toBeNull();
  });

  it("shows how long a settled call took", () => {
    render(
      <ToolCallCard
        item={makeTool({
          status: "done",
          result_content: "ok",
          started_at: 1_000,
          completed_at: 4_200,
        })}
        approval={null}
        onDecide={() => {}}
      />,
    );
    expect(screen.getByText("3s")).toBeInTheDocument();
  });

  it("ticks elapsed time while a call runs", () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(10_000);
      render(
        <ToolCallCard
          item={makeTool({ started_at: 10_000 })}
          turnLive
          approval={null}
          onDecide={() => {}}
        />,
      );
      const elapsed = screen.getByTestId("step-elapsed");
      expect(elapsed.textContent).toBe("");
      act(() => {
        vi.advanceTimersByTime(65_000);
      });
      expect(elapsed.textContent).toBe("1m 5s");
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not tick a call left running in a settled transcript", () => {
    // Allowed, then the app quit before the call reported back.
    render(
      <ToolCallCard
        item={makeTool({ approval_request_id: "req-1", started_at: 10_000 })}
        approval={makePendingApproval({
          resolution: { state: "resolved", decision: { decision: "allow" } },
        })}
        onDecide={() => {}}
      />,
    );
    expect(screen.queryByTestId("step-elapsed")).toBeNull();
  });

  it("times an approval-gated call from its approval, not its request", () => {
    // Raised at 1s, allowed at 121s, finished at 126s: the call ran for 5s.
    render(
      <ToolCallCard
        item={makeTool({
          status: "done",
          result_content: "ok",
          approval_request_id: "req-1",
          started_at: 1_000,
          approved_at: 121_000,
          completed_at: 126_000,
        })}
        approval={makePendingApproval({
          resolution: { state: "resolved", decision: { decision: "allow" } },
        })}
        onDecide={() => {}}
      />,
    );
    expect(screen.getByText("5s")).toBeInTheDocument();
    expect(screen.queryByText("2m 5s")).toBeNull();
  });

  it("opens when an image arrives asynchronously and respects a later manual collapse", () => {
    const onDecide = vi.fn();
    const { rerender } = render(
      <ToolCallCard item={makeTool()} approval={null} onDecide={onDecide} />,
    );
    expect(screen.queryByRole("img")).not.toBeInTheDocument();

    const completed = makeTool({
      status: "done",
      result_content: [
        {
          type: "image",
          source: {
            type: "base64",
            media_type: "image/png",
            data: "AAAA",
          },
        },
      ],
    });
    rerender(
      <ToolCallCard item={completed} approval={null} onDecide={onDecide} />,
    );

    expect(screen.getByRole("img")).toHaveAttribute(
      "src",
      "data:image/png;base64,AAAA",
    );
    fireEvent.click(screen.getByRole("button", { expanded: true }));
    expect(screen.queryByRole("img")).not.toBeInTheDocument();

    rerender(
      <ToolCallCard
        item={makeTool({ status: "done", result_content: null })}
        approval={null}
        onDecide={onDecide}
      />,
    );
    rerender(
      <ToolCallCard
        item={{ ...completed }}
        approval={null}
        onDecide={onDecide}
      />,
    );
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
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
