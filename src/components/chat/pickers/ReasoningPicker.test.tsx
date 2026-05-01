/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ChatModelInfo } from "@/tauri/types";
import { ReasoningPicker } from "./ReasoningPicker";

afterEach(() => {
  // Radix Popover portals content into document.body; React Testing
  // Library's default `render` doesn't clean that up between tests
  // without an explicit `cleanup()`.
  cleanup();
});

// Opus 4.7 — full capability payload. Effort axis populated, context
// axis has two options with `1M` as the production default (highest-
// is-default rule).
const OPUS_4_7: ChatModelInfo = {
  id: "claude-opus-4-7",
  label: "Claude Opus 4.7",
  description: null,
  effort_levels: ["low", "medium", "high", "xhigh", "max"],
  default_effort: "xhigh",
  prompt_injected_effort_levels: ["ultrathink"],
  context_window_options: [
    { value: "200k", label: "200k", is_default: false },
    { value: "1m", label: "1M", is_default: true },
  ],
  supports_adaptive_thinking: true,
  supports_thinking_toggle: false,
  supports_fast_mode: false,
  supports_images: true,
  sub_provider: null,
};

// Haiku — neither axis. Picker should hide entirely.
const HAIKU: ChatModelInfo = {
  ...OPUS_4_7,
  id: "claude-haiku-4-5",
  effort_levels: [],
  default_effort: null,
  prompt_injected_effort_levels: [],
  context_window_options: [],
};

// Effort-only: multiple levels, no context-window choice.
const OPUS_4_5: ChatModelInfo = {
  ...OPUS_4_7,
  id: "claude-opus-4-5",
  effort_levels: ["low", "medium", "high", "max"],
  default_effort: "high",
  prompt_injected_effort_levels: [],
  context_window_options: [],
};

// Context-only edge case: a hypothetical future model that offers
// context-window choice without effort. No current Claude model looks
// like this, but the picker's branches support it — test it.
const CONTEXT_ONLY: ChatModelInfo = {
  ...OPUS_4_7,
  id: "context-only-model",
  effort_levels: [],
  default_effort: null,
  prompt_injected_effort_levels: [],
};

const LABEL_MAP = {
  low: "Low",
  medium: "Medium",
  high: "High",
  xhigh: "Extra High",
  max: "Max",
  ultrathink: "Ultrathink",
};

type Props = Parameters<typeof ReasoningPicker>[0];

function renderPicker(overrides: Partial<Props> = {}) {
  const onEffortChange = vi.fn();
  const onContextWindowChange = vi.fn();
  const utils = render(
    <TooltipProvider>
      <ReasoningPicker
        model={OPUS_4_7}
        effortValue={null}
        contextWindowValue={null}
        labelMap={LABEL_MAP}
        ultrathinkInBodyText={false}
        onEffortChange={onEffortChange}
        onContextWindowChange={onContextWindowChange}
        {...overrides}
      />
    </TooltipProvider>,
  );
  const trigger = utils.container.querySelector("button") as HTMLElement | null;
  return { ...utils, onEffortChange, onContextWindowChange, trigger };
}

describe("ReasoningPicker — render", () => {
  it("hides when model is null (capabilities unavailable)", () => {
    const { container } = renderPicker({ model: null });
    expect(container.querySelector("button")).toBeNull();
  });

  it("hides when the model has no effort AND ≤1 context options (Haiku)", () => {
    const { container } = renderPicker({ model: HAIKU });
    expect(container.querySelector("button")).toBeNull();
  });

  it("renders pill with 'Effort · Context' format when both axes populated", () => {
    const { trigger } = renderPicker({
      effortValue: "xhigh",
      contextWindowValue: "1m",
    });
    expect(trigger).not.toBeNull();
    expect(trigger!.textContent).toContain("Extra High");
    expect(trigger!.textContent).toContain("1M");
    expect(trigger!.textContent).toContain("·");
  });

  it("renders pill with just the effort label when the model has no context axis", () => {
    const { trigger } = renderPicker({
      model: OPUS_4_5,
      effortValue: "high",
    });
    expect(trigger).not.toBeNull();
    expect(trigger!.textContent).toContain("High");
    expect(trigger!.textContent).not.toContain("·");
  });

  it("renders pill with just the context label when the model has no effort axis", () => {
    const { trigger } = renderPicker({
      model: CONTEXT_ONLY,
      contextWindowValue: "200k",
    });
    expect(trigger).not.toBeNull();
    expect(trigger!.textContent).toContain("200k");
    expect(trigger!.textContent).not.toContain("·");
  });

  it("when effortValue is null, pill shows the model's default effort label", () => {
    // Option C fallback: slice null → picker resolves to default.
    // OPUS_4_7's default effort is `xhigh` → "Extra High".
    const { trigger } = renderPicker({
      effortValue: null,
      contextWindowValue: "1m",
    });
    expect(trigger!.textContent).toContain("Extra High");
  });

  it("when contextWindowValue is null, pill shows the default context label", () => {
    // OPUS_4_7's is_default context is `1m` → "1M".
    const { trigger } = renderPicker({
      effortValue: "xhigh",
      contextWindowValue: null,
    });
    expect(trigger!.textContent).toContain("1M");
  });

  it("honors caller-provided non-default values", () => {
    const { trigger } = renderPicker({
      effortValue: "medium",
      contextWindowValue: "200k",
    });
    expect(trigger!.textContent).toContain("Medium");
    expect(trigger!.textContent).toContain("200k");
  });

  it("falls back to default when the slice value isn't a supported option", () => {
    const { trigger } = renderPicker({
      effortValue: "not-a-real-level",
      contextWindowValue: "2g",
    });
    expect(trigger!.textContent).toContain("Extra High");
    expect(trigger!.textContent).toContain("1M");
  });

  it("disabled prop applies to the trigger", () => {
    const { trigger } = renderPicker({ disabled: true });
    expect(trigger!.hasAttribute("disabled")).toBe(true);
  });
});

describe("ReasoningPicker — dropdown structure", () => {
  it("renders Effort + Context Window sections with a separator between them", async () => {
    const user = userEvent.setup();
    const { trigger } = renderPicker();
    await user.click(trigger!);
    // Section headings from cmdk's CommandGroup `heading` prop.
    expect(await screen.findByText("Effort")).toBeInTheDocument();
    expect(await screen.findByText("Context Window")).toBeInTheDocument();
  });

  it("hides Context Window section when model has ≤1 option (Opus 4.5)", async () => {
    const user = userEvent.setup();
    const { trigger } = renderPicker({
      model: OPUS_4_5,
      effortValue: "high",
    });
    await user.click(trigger!);
    expect(await screen.findByText("Effort")).toBeInTheDocument();
    expect(screen.queryByText("Context Window")).toBeNull();
  });

  it("hides Effort section when model has no effort levels", async () => {
    const user = userEvent.setup();
    const { trigger } = renderPicker({
      model: CONTEXT_ONLY,
      contextWindowValue: "1m",
    });
    await user.click(trigger!);
    expect(await screen.findByText("Context Window")).toBeInTheDocument();
    expect(screen.queryByText("Effort")).toBeNull();
  });

  it("lists every native + prompt-injected effort level", async () => {
    const user = userEvent.setup();
    const { trigger } = renderPicker();
    await user.click(trigger!);
    const options = (await screen.findAllByRole("option")).map(
      (el) => el.textContent ?? "",
    );
    for (const label of [
      "Low",
      "Medium",
      "High",
      "Extra High",
      "Max",
      "Ultrathink",
    ]) {
      expect(options.some((t) => t.includes(label))).toBe(true);
    }
  });

  it("lists every context-window option from the model", async () => {
    const user = userEvent.setup();
    const { trigger } = renderPicker();
    await user.click(trigger!);
    const options = (await screen.findAllByRole("option")).map(
      (el) => el.textContent ?? "",
    );
    for (const opt of OPUS_4_7.context_window_options) {
      expect(options.some((t) => t.includes(opt.label))).toBe(true);
    }
  });

  it("marks the default effort with '(default)'", async () => {
    const user = userEvent.setup();
    const { trigger } = renderPicker();
    await user.click(trigger!);
    const defaults = await screen.findAllByText(/\(default\)/);
    // One marker for Extra High (effort default), one for 1M (context default).
    const effortDefault = defaults.find((el) =>
      el.parentElement?.textContent?.includes("Extra High"),
    );
    expect(effortDefault).toBeDefined();
  });

  it("marks the default context window with '(default)'", async () => {
    const user = userEvent.setup();
    const { trigger } = renderPicker();
    await user.click(trigger!);
    const defaults = await screen.findAllByText(/\(default\)/);
    const contextDefault = defaults.find((el) =>
      el.parentElement?.textContent?.includes("1M"),
    );
    expect(contextDefault).toBeDefined();
  });
});

describe("ReasoningPicker — interaction", () => {
  it("clicking an effort row fires onEffortChange with the level id", async () => {
    const user = userEvent.setup();
    const { trigger, onEffortChange } = renderPicker();
    await user.click(trigger!);
    await user.click(await screen.findByText("Medium"));
    expect(onEffortChange).toHaveBeenCalledWith("medium");
  });

  it("clicking Ultrathink fires onEffortChange with 'ultrathink'", async () => {
    const user = userEvent.setup();
    const { trigger, onEffortChange } = renderPicker();
    await user.click(trigger!);
    await user.click(await screen.findByText("Ultrathink"));
    expect(onEffortChange).toHaveBeenCalledWith("ultrathink");
  });

  it("clicking a context-window row fires onContextWindowChange with the value", async () => {
    const user = userEvent.setup();
    const { trigger, onContextWindowChange } = renderPicker({
      contextWindowValue: "1m",
    });
    await user.click(trigger!);
    const opts = await screen.findAllByRole("option");
    const target = opts.find((el) => (el.textContent ?? "").includes("200k"));
    expect(target).toBeDefined();
    await user.click(target!);
    expect(onContextWindowChange).toHaveBeenCalledWith("200k");
  });

  it("ultrathinkInBodyText shows the warning banner", async () => {
    const user = userEvent.setup();
    const { trigger } = renderPicker({ ultrathinkInBodyText: true });
    await user.click(trigger!);
    expect(
      await screen.findByText(/Your prompt contains "ultrathink"/i),
    ).toBeInTheDocument();
  });

  it("ultrathinkInBodyText blocks effort selection — onEffortChange not called", async () => {
    const user = userEvent.setup();
    const { trigger, onEffortChange } = renderPicker({
      ultrathinkInBodyText: true,
    });
    await user.click(trigger!);
    await user.click(await screen.findByText("Low"));
    expect(onEffortChange).not.toHaveBeenCalled();
  });

  it("ultrathinkInBodyText does NOT block context-window selection", async () => {
    // Ultrathink is an effort-axis concept. The context-window rows
    // should remain clickable even when the body contains the token.
    const user = userEvent.setup();
    const { trigger, onContextWindowChange } = renderPicker({
      ultrathinkInBodyText: true,
      contextWindowValue: "1m",
    });
    await user.click(trigger!);
    const opts = await screen.findAllByRole("option");
    const target = opts.find((el) => (el.textContent ?? "").includes("200k"));
    expect(target).toBeDefined();
    await user.click(target!);
    expect(onContextWindowChange).toHaveBeenCalledWith("200k");
  });
});
