/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ChatModelInfo } from "@/tauri/types";
import { ContextWindowPicker } from "./ContextWindowPicker";

afterEach(() => {
  cleanup();
});

const OPUS_4_7: ChatModelInfo = {
  id: "claude-opus-4-7",
  label: "Claude Opus 4.7",
  description: null,
  effort_levels: [],
  default_effort: null,
  prompt_injected_effort_levels: [],
  context_window_options: [
    { value: "200k", label: "200k", is_default: true },
    { value: "1m", label: "1M", is_default: false },
  ],
  supports_adaptive_thinking: false,
  supports_thinking_toggle: false,
  supports_fast_mode: false,
};

const HAIKU: ChatModelInfo = {
  ...OPUS_4_7,
  id: "claude-haiku-4-5",
  context_window_options: [],
};

// Future-proofing: if a model ever adds a 2m option, the picker
// behavior should work with zero changes. This fixture exercises the
// N-options path (not just the 200k/1m pair).
const FUTURE_MODEL: ChatModelInfo = {
  ...OPUS_4_7,
  id: "future-model",
  context_window_options: [
    { value: "200k", label: "200k", is_default: true },
    { value: "1m", label: "1M", is_default: false },
    { value: "2m", label: "2M", is_default: false },
  ],
};

describe("ContextWindowPicker — render", () => {
  it("hides when the model has no options", () => {
    const { container } = render(
      <TooltipProvider>
        <ContextWindowPicker model={HAIKU} value={null} onChange={vi.fn()} />
      </TooltipProvider>,
    );
    expect(container.querySelector("button")).toBeNull();
  });

  it("hides when the user is on the default option (no pill)", () => {
    const { container } = render(
      <TooltipProvider>
        <ContextWindowPicker model={OPUS_4_7} value={"200k"} onChange={vi.fn()} />
      </TooltipProvider>,
    );
    expect(container.querySelector("button")).toBeNull();
  });

  it("hides when value is null (no explicit pick → treated as default)", () => {
    const { container } = render(
      <TooltipProvider>
        <ContextWindowPicker model={OPUS_4_7} value={null} onChange={vi.fn()} />
      </TooltipProvider>,
    );
    expect(container.querySelector("button")).toBeNull();
  });

  it("renders a pill labeled '1M' when the user picked the non-default", () => {
    const { container } = render(
      <TooltipProvider>
        <ContextWindowPicker model={OPUS_4_7} value={"1m"} onChange={vi.fn()} />
      </TooltipProvider>,
    );
    const trigger = container.querySelector("button") as HTMLElement | null;
    expect(trigger).not.toBeNull();
    expect(trigger!.textContent).toContain("1M");
  });

  it("hides when model is null (capabilities unavailable)", () => {
    const { container } = render(
      <TooltipProvider>
        <ContextWindowPicker model={null} value={null} onChange={vi.fn()} />
      </TooltipProvider>,
    );
    expect(container.querySelector("button")).toBeNull();
  });

  it("does NOT render a search input (CommandInput removed)", () => {
    const { container } = render(
      <TooltipProvider>
        <ContextWindowPicker model={OPUS_4_7} value={"1m"} onChange={vi.fn()} />
      </TooltipProvider>,
    );
    expect(
      container.querySelector("input[placeholder*='Context']"),
    ).toBeNull();
  });
});

describe("ContextWindowPicker — interaction", () => {
  it("clicking a non-default option calls onChange with the value", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const { container } = render(
      <TooltipProvider>
        <ContextWindowPicker model={OPUS_4_7} value={"1m"} onChange={onChange} />
      </TooltipProvider>,
    );
    const trigger = container.querySelector("button") as HTMLElement;
    await user.click(trigger);
    // The default option row "200k (default)" should be clickable too
    // — user is on 1M and wants to go back to 200k.
    const opts = await screen.findAllByRole("option");
    const target = opts.find((el) => (el.textContent ?? "").includes("200k"));
    expect(target).toBeDefined();
    await user.click(target!);
    expect(onChange).toHaveBeenCalledWith("200k");
  });

  it("marks the default option with '(default)'", async () => {
    const user = userEvent.setup();
    const { container } = render(
      <TooltipProvider>
        <ContextWindowPicker model={OPUS_4_7} value={"1m"} onChange={vi.fn()} />
      </TooltipProvider>,
    );
    const trigger = container.querySelector("button") as HTMLElement;
    await user.click(trigger);
    const defaultMark = await screen.findByText(/\(default\)/);
    expect(defaultMark.parentElement?.textContent).toMatch(/200k/);
  });

  it("renders every option — scales with model metadata (future 2M etc.)", async () => {
    const user = userEvent.setup();
    const { container } = render(
      <TooltipProvider>
        <ContextWindowPicker
          model={FUTURE_MODEL}
          value={"1m"}
          onChange={vi.fn()}
        />
      </TooltipProvider>,
    );
    const trigger = container.querySelector("button") as HTMLElement;
    await user.click(trigger);
    const optLabels = (await screen.findAllByRole("option")).map(
      (el) => el.textContent ?? "",
    );
    // The test doesn't hardcode "200k/1m" — it pulls labels from the
    // fixture, so adding options in the future requires zero test
    // edits beyond the fixture.
    for (const opt of FUTURE_MODEL.context_window_options) {
      expect(optLabels.some((t) => t.includes(opt.label))).toBe(true);
    }
  });

  it("arrow keys navigate + Enter selects after popover open", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const { container } = render(
      <TooltipProvider>
        <ContextWindowPicker model={OPUS_4_7} value={"1m"} onChange={onChange} />
      </TooltipProvider>,
    );
    const trigger = container.querySelector("button") as HTMLElement;
    await user.click(trigger);
    await screen.findAllByRole("option");
    await user.keyboard("{ArrowDown}{Enter}");
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(["200k", "1m"]).toContain(onChange.mock.calls[0][0] as string);
  });
});
