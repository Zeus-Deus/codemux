/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ComponentProps } from "react";

import { geometryFingerprint } from "@/lib/geometry-fingerprint";
import { TooltipProvider } from "@/components/ui/tooltip";

vi.mock("./pickers/MultiProviderModelPicker", () => ({
  MultiProviderModelPicker: ({
    onProviderModelChange,
    disabled,
  }: {
    onProviderModelChange: (provider: "codex", model: string) => void;
    disabled?: boolean;
  }) => (
    <button
      data-testid="multi-provider-picker-stub"
      onClick={() => onProviderModelChange("codex", "gpt-5.4")}
      disabled={disabled}
    />
  ),
}));

import { ComposerChin, ComposerFooter } from "./ComposerFooter";

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
  it("sends in the background on Ctrl-click and normally on plain click", () => {
    const onSubmit = vi.fn();
    const onBackgroundSubmit = vi.fn();
    renderFooter({ onSubmit, onBackgroundSubmit });
    const send = screen.getByRole("button", { name: "Send" });
    fireEvent.click(send, { ctrlKey: true });
    expect(onBackgroundSubmit).toHaveBeenCalledOnce();
    expect(onSubmit).not.toHaveBeenCalled();
    fireEvent.click(send);
    expect(onSubmit).toHaveBeenCalledOnce();
    fireEvent.click(send, { altKey: true });
    expect(onSubmit).toHaveBeenCalledTimes(2);
    expect(onBackgroundSubmit).toHaveBeenCalledOnce();
    expect(send.title).toContain("Ctrl-click");
  });

  it("does NOT render the legacy '+ Mode' dropdown trigger", () => {
    // The Stage 3 refactor moved mode selection into the unified `+`
    // popup. This test is a regression guard so the dropdown can't
    // creep back.
    renderFooter({ mode: "default" });
    expect(
      screen.queryByRole("button", { name: /Activate mode/i }),
    ).toBeNull();
  });

  it("does NOT render the mode tab (it sits on the card's top edge)", () => {
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

  it("the + button matches the Send button shape", () => {
    renderFooter({ onAttachClick: vi.fn() });
    const attach = screen.getByTestId("composer-attach-button");
    const send = screen.getByRole("button", { name: "Send" });
    // Both share the same circle; identical shape is what makes them read
    // as a visual pair. The pairing is the contract — not the diameter.
    expect(geometryFingerprint(attach)).toBe(geometryFingerprint(send));
    expect(geometryFingerprint(attach)).not.toBe("");
    expect(attach.className).toContain("rounded-full");
  });

  it("leads with the session settings and closes with + and send, around a flexible gap", () => {
    renderFooter({ onAttachClick: vi.fn(), gap: <span>gap text</span> });
    const row = screen.getByTestId("composer-controls-row");
    const gap = screen.getByTestId("composer-gap");
    const attach = screen.getByTestId("composer-attach-button");
    const send = screen.getByRole("button", { name: "Send" });
    expect(gap.className).toContain("flex-1");
    expect(gap).toHaveTextContent("gap text");
    expect(row.firstElementChild).toContainElement(
      screen.getByRole("button", { name: /Full access/i }),
    );
    expect(row.firstElementChild?.nextElementSibling).toBe(gap);
    const right = gap.nextElementSibling;
    expect(right).toContainElement(attach);
    expect(attach.nextElementSibling).toBe(send);
  });

  it("collapsed, keeps only the placeholder, + and send; the chin takes the rest", () => {
    renderFooter({
      collapsed: true,
      onAttachClick: vi.fn(),
      onWorkflowsClick: vi.fn(),
      gap: <span>placeholder</span>,
      contextUsage: { used_tokens: 44_000, max_tokens: 200_000 },
    });
    const row = screen.getByTestId("composer-controls-row");
    expect(row.firstElementChild).toBe(screen.getByTestId("composer-gap"));
    expect(screen.queryByRole("button", { name: /Full access/i })).toBeNull();
    expect(screen.queryByTestId("context-usage-trigger")).toBeNull();
    expect(screen.queryByTestId("composer-workflows-toggle")).toBeNull();
    expect(screen.getByTestId("composer-attach-button")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send" })).toBeInTheDocument();
  });

  it("draws + as a ghost control, with no resting fill", () => {
    renderFooter({ onAttachClick: vi.fn() });
    const attach = screen.getByTestId("composer-attach-button");
    expect(attach.className.split(/\s+/).some((c) => c.startsWith("bg-"))).toBe(false);
  });

  it("width ladder: access goes icon-only, then both leave the row", () => {
    const { rerender } = renderFooter({ accessIconOnly: true });
    const access = screen.getByRole("button", { name: /Access: Full access/i });
    expect(access).not.toHaveTextContent("Full access");

    rerender(
      <TooltipProvider>
        <ComposerFooter {...baseProps()} configInMenu />
      </TooltipProvider>,
    );
    expect(screen.queryByRole("button", { name: /Full access/i })).toBeNull();
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

  it("can freeze configuration without disabling queued-turn attachments", () => {
    renderFooter({
      showProviderPicker: true,
      configurationDisabled: true,
      onAttachClick: vi.fn(),
    });

    expect(screen.getByTestId("multi-provider-picker-stub")).toBeDisabled();
    expect(screen.getByRole("button", { name: /Full access/i })).toBeDisabled();
    expect(screen.getByTestId("composer-attach-button")).not.toBeDisabled();
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

  it("renders the meter at the gap's trailing edge, outside the pinned right cluster", () => {
    renderFooter({
      contextUsage: { used_tokens: 44_000, max_tokens: 200_000 },
    });
    const meter = screen.getByTestId("context-usage-trigger");
    const gap = screen.getByTestId("composer-gap");
    const send = screen.getByRole("button", { name: "Send" });
    expect(meter).toHaveAttribute("aria-label", "Context window 22% used");
    // Inside the gap, so showing / hiding it never moves the controls.
    expect(gap.contains(meter)).toBe(true);
    expect(gap.contains(send)).toBe(false);
  });

  it("stays out of the right cluster while streaming", () => {
    renderFooter({
      streaming: true,
      contextUsage: { used_tokens: 44_000, max_tokens: 200_000 },
    });
    const meter = screen.getByTestId("context-usage-trigger");
    const stop = screen.getByRole("button", { name: "Stop" });
    expect(screen.getByTestId("composer-gap").contains(meter)).toBe(true);
    expect(stop.parentElement?.contains(meter)).toBe(false);
  });

  it("takes the session triggers' 30px hover plate, not the send circle", () => {
    renderFooter({ contextUsage: { used_tokens: 1_000, max_tokens: 200_000 } });
    const meter = screen.getByTestId("context-usage-trigger");
    expect(meter).toHaveClass("size-[30px]", "rounded-md");
    expect(meter.className).not.toContain("rounded-full");
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

describe("message delivery while working", () => {
  it("swaps Stop for the Queue button once there is a draft — never both", () => {
    const onSubmit = vi.fn();
    renderFooter({ streaming: true, onDeliveryChange: vi.fn(), onSubmit });
    expect(screen.getByRole("button", { name: "Message delivery: Queue" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Queue message" }));
    expect(onSubmit).toHaveBeenCalledOnce();
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
  });
  it("shows only Stop while working with an empty draft", () => {
    renderFooter({ streaming: true, canSubmit: false, hasDraft: false, onDeliveryChange: vi.fn() });
    expect(screen.queryByRole("button", { name: "Queue message" })).toBeNull();
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
  });
  it("keeps the send slot (disabled) when a draft exists but can't be sent yet", () => {
    renderFooter({ streaming: true, canSubmit: false, hasDraft: true, onDeliveryChange: vi.fn() });
    expect(screen.getByRole("button", { name: "Queue message" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
  });
  it("offers an explicit interruption separately from safe steering", () => {
    renderFooter({ streaming: true, delivery: "interrupt", onDeliveryChange: vi.fn() });
    expect(screen.getByRole("button", { name: "Interrupt and send" })).toBeInTheDocument();
  });
});

describe("provider steering capability", () => {
  it("explains why safe steering is disabled without hiding queue or interruption", () => {
    renderFooter({ streaming: true, supportsSteering: false, onDeliveryChange: vi.fn() });
    fireEvent.click(screen.getByRole("button", { name: "Message delivery: Queue" }));
    expect(screen.getByRole("option", { name: /Steer \/steer/ })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText(/Safe steering is unavailable for this provider/)).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /^Interrupt and send/ })).not.toHaveAttribute("aria-disabled", "true");
  });
});


describe("ComposerChin", () => {
  function chinProps(): ComponentProps<typeof ComposerChin> {
    const { streaming: _s, canSubmit: _c, onSubmit: _o, onStop: _t, controlsDisabled: _d, ...settings } = baseProps();
    return settings;
  }

  function renderChin(props: Partial<ComponentProps<typeof ComposerChin>> = {}) {
    return render(
      <TooltipProvider>
        <ComposerChin {...chinProps()} {...props} />
      </TooltipProvider>,
    );
  }

  it("carries the session settings, workflows toggle and context ring", () => {
    const onWorkflowsClick = vi.fn();
    renderChin({
      onWorkflowsClick,
      contextUsage: { used_tokens: 44_000, max_tokens: 200_000 },
    });
    const chin = screen.getByTestId("composer-chin");
    expect(chin).toContainElement(screen.getByRole("button", { name: /Full access/i }));
    expect(chin).toContainElement(screen.getByTestId("context-usage-trigger"));
    fireEvent.click(screen.getByTestId("composer-workflows-toggle"));
    expect(onWorkflowsClick).toHaveBeenCalledOnce();
  });

  it("drops labels instead of overflowing a narrow chin", () => {
    // jsdom has no layout: report every element as overflowing.
    const scroll = vi.spyOn(HTMLElement.prototype, "scrollWidth", "get").mockReturnValue(500);
    const client = vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(300);
    try {
      renderChin();
      const access = screen.getByRole("button", { name: /Access: Full access/i });
      expect(access).not.toHaveTextContent("Full access");
    } finally {
      scroll.mockRestore();
      client.mockRestore();
    }
  });
});
