/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, waitFor, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ChatModelInfo } from "@/tauri/types";
import { EffortPicker } from "./EffortPicker";

afterEach(() => {
  // Radix Popover portals content into document.body; React Testing
  // Library's default `render` doesn't clean that up between tests
  // without an explicit `cleanup()`. Without this, every test sees
  // popover items from prior tests → `Found multiple elements…`.
  cleanup();
});

const OPUS_4_7: ChatModelInfo = {
  id: "claude-opus-4-7",
  label: "Claude Opus 4.7",
  description: null,
  effort_levels: ["low", "medium", "high", "xhigh", "max"],
  default_effort: "xhigh",
  prompt_injected_effort_levels: ["ultrathink"],
  context_window_options: [],
  supports_adaptive_thinking: true,
  supports_thinking_toggle: false,
  supports_fast_mode: false,
};

const HAIKU: ChatModelInfo = {
  ...OPUS_4_7,
  id: "claude-haiku-4-5",
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

function renderPicker(overrides: Partial<Parameters<typeof EffortPicker>[0]> = {}) {
  const onChange = vi.fn();
  const utils = render(
    <TooltipProvider>
      <EffortPicker
        model={OPUS_4_7}
        value={null}
        labelMap={LABEL_MAP}
        ultrathinkInBodyText={false}
        onChange={onChange}
        {...overrides}
      />
    </TooltipProvider>,
  );
  const trigger = utils.container.querySelector("button") as HTMLElement | null;
  return { ...utils, onChange, trigger };
}

describe("EffortPicker — render", () => {
  it("renders a trigger pill when the model supports effort", () => {
    const { trigger } = renderPicker();
    expect(trigger).not.toBeNull();
    expect(trigger!.textContent).toContain("Extra High");
  });

  it("returns null when the model has no effort levels (Haiku)", () => {
    const { container } = renderPicker({ model: HAIKU });
    expect(container.querySelector("button")).toBeNull();
  });

  it("returns null when the model is null (capabilities unavailable)", () => {
    const { container } = renderPicker({ model: null });
    expect(container.querySelector("button")).toBeNull();
  });

  it("uses the model default when value is unknown", () => {
    const { trigger } = renderPicker({ value: "not-a-real-level" });
    expect(trigger!.textContent).toContain("Extra High");
  });

  it("honors the caller-provided value when supported", () => {
    const { trigger } = renderPicker({ value: "medium" });
    expect(trigger!.textContent).toContain("Medium");
  });

  it("shows 'Ultrathink' in the trigger when value is ultrathink", () => {
    const { trigger } = renderPicker({ value: "ultrathink" });
    expect(trigger!.textContent).toContain("Ultrathink");
  });

  it("disabled prop prevents opening", () => {
    const { trigger } = renderPicker({ disabled: true });
    expect(trigger!.hasAttribute("disabled")).toBe(true);
  });

  it("does NOT render a search input (CommandInput removed)", () => {
    const { container } = renderPicker();
    expect(
      container.querySelector("input[placeholder*='Search']"),
    ).toBeNull();
  });
});

describe("EffortPicker — interaction", () => {
  it("lists every native + prompt-injected level in the menu", async () => {
    const user = userEvent.setup();
    const { trigger } = renderPicker();
    await user.click(trigger!);
    await waitFor(() => {
      // `getAllByRole` → any option-role element. Trigger pill text
      // overlaps menu text ("Extra High" appears in both), so we count
      // occurrences on the menu items rather than on text nodes.
      expect(screen.getAllByRole("option").length).toBeGreaterThan(0);
    });
    const options = screen
      .getAllByRole("option")
      .map((el) => el.textContent ?? "");
    // Every level label should appear inside at least one option row.
    for (const label of ["Low", "Medium", "High", "Extra High", "Max", "Ultrathink"]) {
      expect(options.some((t) => t.includes(label))).toBe(true);
    }
  });

  it("clicking a normal level calls onChange with that id", async () => {
    const user = userEvent.setup();
    const { trigger, onChange } = renderPicker();
    await user.click(trigger!);
    await user.click(await screen.findByText("Medium"));
    expect(onChange).toHaveBeenCalledWith("medium");
  });

  it("clicking Ultrathink calls onChange with 'ultrathink'", async () => {
    const user = userEvent.setup();
    const { trigger, onChange } = renderPicker();
    await user.click(trigger!);
    await user.click(await screen.findByText("Ultrathink"));
    expect(onChange).toHaveBeenCalledWith("ultrathink");
  });

  it("ultrathinkInBodyText shows the warning message", async () => {
    const user = userEvent.setup();
    const { trigger } = renderPicker({ ultrathinkInBodyText: true });
    await user.click(trigger!);
    await waitFor(() => {
      expect(
        screen.getByText(/Your prompt contains "ultrathink"/i),
      ).toBeInTheDocument();
    });
  });

  it("ultrathinkInBodyText blocks selection — onChange is not called", async () => {
    const user = userEvent.setup();
    const { trigger, onChange } = renderPicker({ ultrathinkInBodyText: true });
    await user.click(trigger!);
    const lowItem = await screen.findByText("Low");
    await user.click(lowItem);
    expect(onChange).not.toHaveBeenCalled();
  });

  it("marks the default effort with '(default)'", async () => {
    const user = userEvent.setup();
    const { trigger } = renderPicker();
    await user.click(trigger!);
    const defaultMark = await screen.findByText(/\(default\)/);
    // The default effort is xhigh for OPUS_4_7 → "Extra High".
    expect(defaultMark.parentElement?.textContent).toMatch(/Extra High/);
  });

  it("arrow keys navigate + Enter selects after popover open (no CommandInput)", async () => {
    const user = userEvent.setup();
    const { trigger, onChange } = renderPicker();
    await user.click(trigger!);
    await screen.findAllByRole("option");
    // Arrow down twice: first press highlights the default ("xhigh"),
    // second press advances one row.
    await user.keyboard("{ArrowDown}{ArrowDown}");
    await user.keyboard("{Enter}");
    expect(onChange).toHaveBeenCalledTimes(1);
    const picked = onChange.mock.calls[0][0];
    // Must be SOME native level — proves cmdk received the keydown.
    expect(
      [
        "low",
        "medium",
        "high",
        "xhigh",
        "max",
        "ultrathink",
      ].includes(picked as string),
    ).toBe(true);
  });
});
