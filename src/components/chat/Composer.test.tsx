/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";
import type { ComponentProps } from "react";
import { TooltipProvider } from "@/components/ui/tooltip";

import { Composer } from "./Composer";
import type { ChatModelInfo } from "@/tauri/types";

type ComposerProps = ComponentProps<typeof Composer>;

afterEach(() => cleanup());

function baseProps(): ComposerProps {
  return {
    draft: "",
    cwd: "/home/user",
    provider: "claude",
    model: null,
    permissionMode: null,
    effort: null,
    contextWindow: null,
    activeModel: null,
    effortLabelMap: {},
    permissionModes: null,
    ultrathinkInBodyText: false,
    streaming: false,
    sessionReady: true,
    showProviderPicker: false,
    mode: "default",
    onDraftChange: vi.fn(),
    onSubmit: vi.fn(),
    onStop: vi.fn(),
    onProviderChange: vi.fn(),
    onModelChange: vi.fn(),
    onPermissionModeChange: vi.fn(),
    onEffortChange: vi.fn(),
    onContextWindowChange: vi.fn(),
    onModeActivate: vi.fn(),
    onModeRemove: vi.fn(),
  };
}

function renderComposer(props: Partial<ComposerProps> = {}) {
  return render(
    <TooltipProvider>
      <Composer {...baseProps()} {...props} />
    </TooltipProvider>,
  );
}

describe("Composer", () => {
  describe("error banner (§8)", () => {
    it("renders the banner when errorMessage is set", () => {
      const { getByRole } = renderComposer({
        errorMessage: "workspace creation failed",
      });
      const alert = getByRole("alert");
      expect(alert.textContent).toContain("Send failed");
      expect(alert.textContent).toContain("workspace creation failed");
      expect(alert.textContent).toContain("Press Enter to retry");
    });

    it("does not render the banner when errorMessage is null", () => {
      const { queryByRole } = renderComposer({ errorMessage: null });
      expect(queryByRole("alert")).toBeNull();
    });

    it("does not render the banner when errorMessage is undefined (omitted)", () => {
      const { queryByRole } = renderComposer();
      expect(queryByRole("alert")).toBeNull();
    });
  });

  describe("showStopButton (§6.5)", () => {
    it("hides the Stop button while streaming when showStopButton=false", () => {
      const { container } = renderComposer({
        streaming: true,
        showStopButton: false,
      });
      expect(container.querySelector('button[aria-label="Stop"]')).toBeNull();
      // The Send button remains visible but disabled, so the draft
      // composer still reads as 'working…' to the user.
      const send = container.querySelector(
        'button[aria-label="Send"]',
      ) as HTMLButtonElement | null;
      expect(send).not.toBeNull();
      expect(send!.disabled).toBe(true);
    });

    it("renders the Stop button while streaming by default", () => {
      const { container } = renderComposer({ streaming: true });
      expect(container.querySelector('button[aria-label="Stop"]')).not.toBeNull();
    });
  });

  describe("zone1Override (§12)", () => {
    it("replaces the default cwd strip when provided", () => {
      const { container, queryByText } = renderComposer({
        zone1Override: <button aria-label="custom-picker">Pick a project</button>,
        cwd: "/home/user/projects/foo",
      });
      // Default cwd label hidden.
      expect(queryByText("/home/user/projects/foo")).toBeNull();
      // Override rendered in the Zone 1 slot.
      expect(
        container.querySelector('button[aria-label="custom-picker"]'),
      ).not.toBeNull();
    });

    it("falls back to the cwd label when no override is provided", () => {
      const { getByText } = renderComposer({
        cwd: "/home/user/projects/foo",
      });
      expect(getByText("/home/user/projects/foo")).toBeInTheDocument();
    });

    it("hides the cwd strip when both override and cwd are absent", () => {
      const { container } = renderComposer({ cwd: null });
      // Nothing rendered above the textarea.
      expect(container.textContent).not.toContain("Home");
    });
  });

  describe("post-materialize picker visibility (Stage C Effort-lock fix)", () => {
    // A capability payload mimicking what `provider-capabilities-store`
    // returns for Claude Opus 4.7 after Rust hydration — the draft
    // materialize flow seeds this onto the slice.
    const fullActiveModel: ChatModelInfo = {
      id: "claude-opus-4-7",
      label: "Claude Opus 4.7",
      description: "Strongest Claude model",
      effort_levels: ["low", "medium", "high", "xhigh", "max"],
      default_effort: "xhigh",
      prompt_injected_effort_levels: ["ultrathink"],
      // Highest-is-default rule: 1M is flagged default in
      // production. With always-render the picker shows up either
      // way, so the test below deliberately exercises a non-default
      // selection to keep assertions discriminating.
      context_window_options: [
        { value: "200k", label: "200k", is_default: false },
        { value: "1m", label: "1M", is_default: true },
      ],
      supports_adaptive_thinking: true,
      supports_thinking_toggle: false,
      supports_fast_mode: false,
    };

    it("renders ReasoningPicker when activeModel is populated (not null)", () => {
      const { container } = renderComposer({
        model: "claude-opus-4-7",
        effort: "xhigh",
        activeModel: fullActiveModel,
        effortLabelMap: {
          low: "Low",
          medium: "Medium",
          high: "High",
          xhigh: "Extra High",
          max: "Max",
          ultrathink: "Ultrathink",
        },
      });
      const buttons = Array.from(
        container.querySelectorAll<HTMLButtonElement>("button"),
      );
      // ReasoningPicker's trigger shows the combined "Effort · Context"
      // label ("Extra High · 1M" here — contextWindow falls back to
      // the model's is_default via Option C). Presence of that text is
      // proof the picker rendered rather than returning null.
      const reasoningTrigger = buttons.find((b) =>
        b.textContent?.includes("Extra High"),
      );
      expect(reasoningTrigger).toBeDefined();
      expect(reasoningTrigger?.disabled).toBe(false);
    });

    it("renders the context-window label in the ReasoningPicker pill, even on the default", () => {
      // Always-render rule (Stage C follow-up): the pill shows up
      // whether the user is on the default (1M) or on a non-default
      // option. Verify both.
      const onDefault = renderComposer({
        model: "claude-opus-4-7",
        contextWindow: "1m",
        activeModel: fullActiveModel,
      });
      expect(onDefault.container.textContent).toContain("1M");
      cleanup();
      const onNonDefault = renderComposer({
        model: "claude-opus-4-7",
        contextWindow: "200k",
        activeModel: fullActiveModel,
      });
      expect(onNonDefault.container.textContent).toContain("200k");
    });

    it("hides both pickers when activeModel is null (regression guard)", () => {
      const { container } = renderComposer({
        model: null,
        activeModel: null,
      });
      // Neither "Extra High" nor "200k" should appear anywhere.
      expect(container.textContent).not.toContain("Extra High");
      expect(container.textContent).not.toContain("200k");
    });
  });

  describe("Stage 3 — mode pill integration", () => {
    it("swaps the textarea placeholder to match the active mode", () => {
      const { getByPlaceholderText } = renderComposer({
        mode: "plan",
        sessionReady: true,
      });
      expect(
        getByPlaceholderText("Plan and design before coding…"),
      ).toBeInTheDocument();
    });

    it("keeps the default placeholder when mode is default", () => {
      const { getByPlaceholderText } = renderComposer({
        mode: "default",
        sessionReady: true,
      });
      expect(getByPlaceholderText("Message the agent…")).toBeInTheDocument();
    });

    it("'/plan ' at draft start activates Plan mode and strips the command", () => {
      const onDraftChange = vi.fn();
      const onModeActivate = vi.fn();
      const { container } = renderComposer({
        mode: "default",
        onDraftChange,
        onModeActivate,
      });
      const textarea = container.querySelector(
        "textarea",
      ) as HTMLTextAreaElement;
      // Simulate the user typing "/plan refactor me" into the empty
      // textarea. The ChangeEvent reports the full composed value.
      textarea.dispatchEvent(
        new (window as unknown as { Event: typeof Event }).Event("input", {
          bubbles: true,
        }),
      );
      // The simpler path: fire a native change via React Testing Library.
      // Simulating a change via dispatchEvent + value set covers the
      // React controlled-input path.
      const nativeInputValueSetter = Object.getOwnPropertyDescriptor(
        window.HTMLTextAreaElement.prototype,
        "value",
      )?.set;
      nativeInputValueSetter?.call(textarea, "/plan refactor me");
      textarea.dispatchEvent(new Event("input", { bubbles: true }));

      expect(onModeActivate).toHaveBeenCalledWith("plan");
      expect(onDraftChange).toHaveBeenCalledWith("refactor me");
    });

    it("'/plan' on its own strips to empty and activates", () => {
      const onDraftChange = vi.fn();
      const onModeActivate = vi.fn();
      const { container } = renderComposer({
        mode: "default",
        onDraftChange,
        onModeActivate,
      });
      const textarea = container.querySelector(
        "textarea",
      ) as HTMLTextAreaElement;
      const setter = Object.getOwnPropertyDescriptor(
        window.HTMLTextAreaElement.prototype,
        "value",
      )?.set;
      setter?.call(textarea, "/plan");
      textarea.dispatchEvent(new Event("input", { bubbles: true }));

      expect(onModeActivate).toHaveBeenCalledWith("plan");
      expect(onDraftChange).toHaveBeenCalledWith("");
    });

    it("slash-commands are ignored mid-draft (only at position 0)", () => {
      const onDraftChange = vi.fn();
      const onModeActivate = vi.fn();
      const { container } = renderComposer({
        mode: "default",
        draft: "hello ",
        onDraftChange,
        onModeActivate,
      });
      const textarea = container.querySelector(
        "textarea",
      ) as HTMLTextAreaElement;
      const setter = Object.getOwnPropertyDescriptor(
        window.HTMLTextAreaElement.prototype,
        "value",
      )?.set;
      setter?.call(textarea, "hello /plan world");
      textarea.dispatchEvent(new Event("input", { bubbles: true }));

      expect(onModeActivate).not.toHaveBeenCalled();
      // Passed through to the parent verbatim.
      expect(onDraftChange).toHaveBeenCalledWith("hello /plan world");
    });

    it("slash-commands are ignored when a mode pill is already active", () => {
      const onModeActivate = vi.fn();
      const onDraftChange = vi.fn();
      const { container } = renderComposer({
        mode: "plan",
        onDraftChange,
        onModeActivate,
      });
      const textarea = container.querySelector(
        "textarea",
      ) as HTMLTextAreaElement;
      const setter = Object.getOwnPropertyDescriptor(
        window.HTMLTextAreaElement.prototype,
        "value",
      )?.set;
      setter?.call(textarea, "/plan twice");
      textarea.dispatchEvent(new Event("input", { bubbles: true }));

      expect(onModeActivate).not.toHaveBeenCalled();
      // Text passes through so the user can still type literal /.
      expect(onDraftChange).toHaveBeenCalledWith("/plan twice");
    });

    it("'/ask' strips the command but does not activate in Stage 3", () => {
      const onDraftChange = vi.fn();
      const onModeActivate = vi.fn();
      const { container } = renderComposer({
        mode: "default",
        onDraftChange,
        onModeActivate,
      });
      const textarea = container.querySelector(
        "textarea",
      ) as HTMLTextAreaElement;
      const setter = Object.getOwnPropertyDescriptor(
        window.HTMLTextAreaElement.prototype,
        "value",
      )?.set;
      setter?.call(textarea, "/ask when does the release ship?");
      textarea.dispatchEvent(new Event("input", { bubbles: true }));

      // Stage 3: /ask is reserved — strip the command but do not fire
      // onModeActivate (Stage 4 will wire the prompt-wrapper path).
      expect(onModeActivate).not.toHaveBeenCalled();
      expect(onDraftChange).toHaveBeenCalledWith(
        "when does the release ship?",
      );
    });
  });
});
