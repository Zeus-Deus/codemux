/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ComponentProps } from "react";

import { TooltipProvider } from "@/components/ui/tooltip";

vi.mock("./pickers/MultiProviderModelPicker", () => ({
  MultiProviderModelPicker: ({
    onProviderModelChange,
  }: {
    onProviderModelChange: (provider: "codex", model: string) => void;
  }) => (
    <button
      data-testid="multi-provider-picker-stub"
      onClick={() => onProviderModelChange("codex", "gpt-5.4")}
    />
  ),
}));

import { ComposerFooter } from "./ComposerFooter";

type FooterProps = ComponentProps<typeof ComposerFooter>;

afterEach(() => cleanup());

function baseProps(): FooterProps {
  return {
    provider: "claude",
    model: null,
    permissionMode: "bypassPermissions",
    effort: null,
    contextWindow: null,
    activeModel: null,
    effortLabelMap: {},
    permissionModes: [
      {
        value: "bypassPermissions",
        label: "Full access",
        description: "",
        is_default: true,
      },
      {
        value: "default",
        label: "Supervised",
        description: "",
        is_default: false,
      },
    ],
    ultrathinkInBodyText: false,
    streaming: false,
    canSubmit: true,
    showProviderPicker: false,
    mode: "default",
    onProviderModelChange: vi.fn(),
    onModelChange: vi.fn(),
    onPermissionModeChange: vi.fn(),
    onEffortChange: vi.fn(),
    onContextWindowChange: vi.fn(),
    onSubmit: vi.fn(),
    onStop: vi.fn(),
    controlsDisabled: false,
  };
}

function renderFooter(props: Partial<FooterProps> = {}) {
  return render(
    <TooltipProvider>
      <ComposerFooter {...baseProps()} {...props} />
    </TooltipProvider>,
  );
}

describe("ComposerFooter — Stage 3 refactor (unified + popup)", () => {
  it("does NOT render the legacy '+ Mode' dropdown trigger", () => {
    // The Stage 3 refactor moved mode selection into the unified `+`
    // popup. This test is a regression guard so the dropdown can't
    // creep back.
    renderFooter({ mode: "default" });
    expect(
      screen.queryByRole("button", { name: /Activate mode/i }),
    ).toBeNull();
  });

  it("does NOT render an inline ModePill (it lives above the textarea now)", () => {
    renderFooter({ mode: "plan" });
    expect(
      screen.queryByRole("status", { name: /Plan mode active/i }),
    ).toBeNull();
  });

  it("renders the + button when onAttachClick is provided", () => {
    renderFooter({ onAttachClick: vi.fn() });
    const btn = screen.getByTestId("composer-attach-button");
    expect(btn).toBeInTheDocument();
  });

  it("the + button matches the Send button shape (h-8 w-8 circle)", () => {
    renderFooter({ onAttachClick: vi.fn() });
    const attach = screen.getByTestId("composer-attach-button");
    const send = screen.getByRole("button", { name: "Send" });
    // Both share the same fixed circle dimensions; identical shape
    // is what makes them read as a visual pair.
    expect(attach.className).toContain("h-8");
    expect(attach.className).toContain("w-8");
    expect(attach.className).toContain("rounded-full");
    expect(send.className).toContain("h-8");
    expect(send.className).toContain("w-8");
    expect(send.className).toContain("rounded-full");
  });

  it("the + button is hidden when onAttachClick is omitted (back-compat)", () => {
    renderFooter();
    expect(screen.queryByTestId("composer-attach-button")).toBeNull();
  });

  it("PermissionModePicker is enabled in default mode", () => {
    renderFooter({ mode: "default" });
    const picker = screen.getByRole("button", { name: /Full access/i });
    expect(picker).not.toBeDisabled();
  });

  it("PermissionModePicker stays visible but disabled when a mode is active", () => {
    // Plan / Ask / Debug commandeer permission mode at the SDK
    // boundary; the picker stays on-screen for discoverability but
    // can't override the pill that's driving the policy.
    renderFooter({ mode: "plan" });
    const picker = screen.getByRole("button", { name: /Full access/i });
    expect(picker).toBeInTheDocument();
    expect(picker).toBeDisabled();
  });

  it("the Send button is rendered and enabled when canSubmit is true", () => {
    renderFooter({ canSubmit: true });
    const send = screen.getByRole("button", { name: "Send" });
    expect(send).not.toBeDisabled();
  });

  it("the Stop button replaces Send while streaming", () => {
    renderFooter({ streaming: true });
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Send" })).toBeNull();
  });

  it("the streaming Stop button uses the destructive (red) treatment", () => {
    // Design D10: the interrupt is the one destructive action in the
    // footer, so it reads red rather than the neutral Send fill.
    renderFooter({ streaming: true });
    const stop = screen.getByRole("button", { name: "Stop" });
    expect(stop.className).toContain("bg-destructive");
    expect(stop.className).toContain("rounded-full");
  });

  it("controlsDisabled disables the + button", () => {
    renderFooter({ controlsDisabled: true, onAttachClick: vi.fn() });
    const attach = screen.getByTestId("composer-attach-button");
    expect(attach).toBeDisabled();
  });

  it("forwards a cross-provider model pick as one atomic selection", () => {
    const onProviderModelChange = vi.fn();
    const onModelChange = vi.fn();
    renderFooter({
      showProviderPicker: true,
      onProviderModelChange,
      onModelChange,
    });

    fireEvent.click(screen.getByTestId("multi-provider-picker-stub"));

    expect(onProviderModelChange).toHaveBeenCalledWith("codex", "gpt-5.4");
    expect(onModelChange).not.toHaveBeenCalled();
  });

  it("reveals the Tasks toggle only for a non-empty provider plan", () => {
    const onTasksClick = vi.fn();
    const { rerender } = renderFooter();
    expect(screen.queryByTestId("composer-tasks-toggle")).toBeNull();

    rerender(
      <TooltipProvider>
        <ComposerFooter
          {...baseProps()}
          tasks={{ completed: 2, total: 5 }}
          onTasksClick={onTasksClick}
        />
      </TooltipProvider>,
    );
    const toggle = screen.getByTestId("composer-tasks-toggle");
    expect(toggle).toHaveTextContent("Tasks");
    expect(toggle).toHaveTextContent("2/5");
    fireEvent.click(toggle);
    expect(onTasksClick).toHaveBeenCalledOnce();
  });

  it("marks the Tasks toggle pressed while its panel is open", () => {
    renderFooter({
      tasks: { completed: 0, total: 3 },
      tasksOpen: true,
      onTasksClick: vi.fn(),
    });
    expect(screen.getByTestId("composer-tasks-toggle")).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  });

  it.each([
    ["waiting", { completed: 0, total: 3, running: false }],
    ["running", { completed: 1, total: 3, running: true }],
    ["complete", { completed: 3, total: 3, running: false }],
  ])("keeps the %s Tasks treatment borderless", (_state, tasks) => {
    renderFooter({ tasks, onTasksClick: vi.fn() });

    const toggle = screen.getByTestId("composer-tasks-toggle");
    const classes = toggle.className.split(/\s+/);
    expect(toggle).toHaveClass("border-0");
    expect(
      classes.some((className) =>
        className === "border" ||
        (className.startsWith("border-") && className !== "border-0"),
      ),
    ).toBe(false);
  });
});

describe("ComposerFooter — context-window meter", () => {
  it("renders no meter when the thread has no usage snapshot", () => {
    // Back-compat: every pre-existing call site omits these props, and
    // the footer's right cluster must stay exactly as it was.
    renderFooter();
    expect(screen.queryByTestId("context-usage-trigger")).toBeNull();
  });

  it("renders no meter when usage is explicitly null", () => {
    renderFooter({ contextUsage: null });
    expect(screen.queryByTestId("context-usage-trigger")).toBeNull();
  });

  it("renders the meter immediately before the Send button", () => {
    renderFooter({
      contextUsage: { used_tokens: 44_000, max_tokens: 200_000 },
    });
    const meter = screen.getByTestId("context-usage-trigger");
    const send = screen.getByRole("button", { name: "Send" });
    expect(meter).toBeInTheDocument();
    // Same cluster, meter first — the readout is passed on the way to
    // the send control.
    expect(meter.parentElement).toBe(send.parentElement);
    expect(meter.nextElementSibling).toBe(send);
    expect(meter).toHaveAttribute("aria-label", "Context window 22% used");
  });

  it("sits before the Stop button while streaming", () => {
    renderFooter({
      streaming: true,
      contextUsage: { used_tokens: 44_000, max_tokens: 200_000 },
    });
    const meter = screen.getByTestId("context-usage-trigger");
    const stop = screen.getByRole("button", { name: "Stop" });
    expect(meter.nextElementSibling).toBe(stop);
  });

  it("matches the send button's circle shape", () => {
    renderFooter({ contextUsage: { used_tokens: 1_000, max_tokens: 200_000 } });
    const meter = screen.getByTestId("context-usage-trigger");
    expect(meter.className).toContain("h-8");
    expect(meter.className).toContain("w-8");
    expect(meter.className).toContain("rounded-full");
  });

  it("uses the capability seed when the snapshot has no window", () => {
    renderFooter({
      contextUsage: { used_tokens: 44_000 },
      contextUsageSeedMaxTokens: 200_000,
    });
    expect(screen.getByTestId("context-usage-trigger")).toHaveAttribute(
      "aria-label",
      "Context window 22% used",
    );
  });

  it("forwards the provider label into the compaction note", () => {
    renderFooter({
      contextUsage: { used_tokens: 44_000, compacts_automatically: true },
      contextUsageProviderLabel: "Claude",
    });
    fireEvent.click(screen.getByTestId("context-usage-trigger"));
    expect(
      screen.getByText(/Claude automatically compacts its context when needed/),
    ).toBeInTheDocument();
  });
});
