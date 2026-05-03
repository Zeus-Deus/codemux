/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import type { PermissionRequestItem } from "@/lib/agent-chat/types";

import { PlanProposalBlock } from "./PlanProposalBlock";

afterEach(() => cleanup());

function makePlanItem(
  overrides: Partial<PermissionRequestItem> = {},
): PermissionRequestItem {
  return {
    kind: "permission_request",
    id: "req-plan-1",
    seq: 0,
    request_id: "req-plan-1",
    turn_id: "turn-1",
    request_kind: "plan",
    payload: {
      plan: "# Refactor\n\n- Step one\n- Step two",
    },
    tool_use_id: "tu-plan-1",
    resolution: { state: "pending" },
    ...overrides,
  };
}

describe("PlanProposalBlock", () => {
  it("renders the plan body via ChatMarkdown (headings as prose, bullets as list)", () => {
    render(
      <PlanProposalBlock
        item={makePlanItem()}
        onAccept={vi.fn()}
        onReject={vi.fn()}
      />,
    );
    // Heading renders as emphasized prose text, not a heading tag.
    expect(screen.getByText("Refactor")).toBeInTheDocument();
    // Bullet list items are rendered.
    expect(screen.getByText("Step one")).toBeInTheDocument();
    expect(screen.getByText("Step two")).toBeInTheDocument();
    // And the subtle "Plan proposed" label is present.
    expect(screen.getByText("Plan proposed")).toBeInTheDocument();
  });

  it("Accept calls onAccept", async () => {
    const onAccept = vi.fn().mockResolvedValue(undefined);
    render(
      <PlanProposalBlock
        item={makePlanItem()}
        onAccept={onAccept}
        onReject={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByText("Accept & execute"));
    expect(onAccept).toHaveBeenCalled();
  });

  it("Reject fires onReject immediately without any follow-up popup", async () => {
    const onReject = vi.fn().mockResolvedValue(undefined);
    render(
      <PlanProposalBlock
        item={makePlanItem()}
        onAccept={vi.fn()}
        onReject={onReject}
      />,
    );
    fireEvent.click(screen.getByText("Reject"));
    expect(onReject).toHaveBeenCalledTimes(1);
    expect(onReject).toHaveBeenCalledWith();
    // No feedback textarea or Send feedback button should appear —
    // matches Cursor / VS Code plan UIs.
    expect(
      screen.queryByPlaceholderText(/What should the plan do differently/),
    ).toBeNull();
    expect(screen.queryByText("Send feedback")).toBeNull();
  });

  it("falls back to raw JSON dump when payload.plan is not a string", () => {
    render(
      <PlanProposalBlock
        item={makePlanItem({
          // Defensive path: SDK drift shouldn't drop the proposal.
          payload: { plan: { structured: "weird" }, notes: "nope" },
        })}
        onAccept={vi.fn()}
        onReject={vi.fn()}
      />,
    );
    // Raw payload text surfaces inside the tier-3 monospace block.
    expect(screen.getByText(/"structured"/)).toBeInTheDocument();
    expect(screen.getByText(/"weird"/)).toBeInTheDocument();
  });

  it("renders the allowedPrompts footer when present", () => {
    render(
      <PlanProposalBlock
        item={makePlanItem({
          payload: {
            plan: "Do the thing.",
            allowedPrompts: [
              { tool: "Bash", prompt: "run tests" },
              { tool: "Bash", prompt: "install dependencies" },
            ],
          },
        })}
        onAccept={vi.fn()}
        onReject={vi.fn()}
      />,
    );
    expect(screen.getByText("Will run:")).toBeInTheDocument();
    expect(screen.getByText(/Bash:.*run tests/)).toBeInTheDocument();
    expect(screen.getByText(/Bash:.*install dependencies/)).toBeInTheDocument();
  });

  it("omits the allowedPrompts footer when absent or invalid", () => {
    const { container } = render(
      <PlanProposalBlock
        item={makePlanItem()}
        onAccept={vi.fn()}
        onReject={vi.fn()}
      />,
    );
    expect(container.textContent).not.toContain("Will run:");
  });

  it("collapses to a one-liner once the resolution is no longer pending", () => {
    const item = makePlanItem({
      resolution: { state: "responding", decision: { decision: "allow" } },
    });
    render(
      <PlanProposalBlock
        item={item}
        onAccept={vi.fn()}
        onReject={vi.fn()}
      />,
    );
    expect(screen.queryByText("Accept & execute")).toBeNull();
    expect(screen.getByText(/Submitting decision/)).toBeInTheDocument();
  });

  it("disables buttons after Accept submits so a double-click is a no-op", () => {
    const onAccept = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          setTimeout(resolve, 50);
        }),
    );
    render(
      <PlanProposalBlock
        item={makePlanItem()}
        onAccept={onAccept}
        onReject={vi.fn()}
      />,
    );
    const btn = screen.getByText("Accept & execute") as HTMLButtonElement;
    fireEvent.click(btn);
    // Fire again immediately — still disabled, handler only invoked once.
    fireEvent.click(btn);
    expect(onAccept).toHaveBeenCalledTimes(1);
  });
});
