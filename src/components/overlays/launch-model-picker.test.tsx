/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { LaunchModelPicker } from "./launch-model-picker";
import type { LaunchModel } from "@/lib/launch-models";
import { usePickerFavorites } from "@/stores/picker-favorites-store";

const CLAUDE_MODELS: LaunchModel[] = [
  { id: "opus", label: "Claude Opus" },
  { id: "sonnet", label: "Claude Sonnet" },
  { id: "haiku", label: "Claude Haiku" },
];

// Reasoning / context option shapes the dialog now derives live from
// the capability bundle; the picker just renders whatever it is given.
const CLAUDE_REASONING = [
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
  { value: "xhigh", label: "Extra High" },
];

const CONTEXT_OPTIONS = [
  { value: "200k", label: "200k" },
  { value: "1m", label: "1M" },
];

function manyModels(count: number): LaunchModel[] {
  return Array.from({ length: count }, (_, i) => ({
    id: `model-${i}`,
    label: `Model ${i}`,
    subProvider: i % 2 === 0 ? "openai" : "anthropic",
  }));
}

type PickerProps = Parameters<typeof LaunchModelPicker>[0];

/** Render the picker with sensible defaults; override per test. */
function renderPicker(overrides: Partial<PickerProps> = {}) {
  const props: PickerProps = {
    providerKind: "claude",
    models: CLAUDE_MODELS,
    selectedModel: null,
    selectedReasoning: null,
    reasoningOptions: [],
    contextOptions: [],
    selectedContext: null,
    onModelChange: vi.fn(),
    onReasoningChange: vi.fn(),
    onContextChange: vi.fn(),
    ...overrides,
  };
  const result = render(<LaunchModelPicker {...props} />);
  const trigger = result.container.querySelector("button") as HTMLElement;
  return { ...result, props, trigger };
}

beforeEach(() => {
  usePickerFavorites.setState({ favorites: [] });
});

afterEach(() => {
  cleanup();
});

describe("LaunchModelPicker — pill label", () => {
  it("shows 'Default' when no model is selected", () => {
    const { trigger } = renderPicker();
    expect(trigger.textContent).toContain("Default");
  });

  it("shows the model label plus reasoning and context suffixes", () => {
    const { trigger } = renderPicker({
      selectedModel: "sonnet",
      selectedReasoning: "high",
      reasoningOptions: CLAUDE_REASONING,
      contextOptions: CONTEXT_OPTIONS,
      selectedContext: "1m",
    });
    expect(trigger.textContent).toContain("Claude Sonnet");
    expect(trigger.textContent).toContain("High");
    expect(trigger.textContent).toContain("1M");
  });
});

describe("LaunchModelPicker — interaction", () => {
  it("opens the popover and reports the picked model", async () => {
    const onModelChange = vi.fn();
    const { trigger } = renderPicker({ onModelChange });
    await userEvent.click(trigger);
    await userEvent.click(screen.getByText("Claude Opus"));
    expect(onModelChange).toHaveBeenCalledWith("opus");
  });

  it("has a single 'Default' row that resets model, reasoning, and context", async () => {
    const onModelChange = vi.fn();
    const onReasoningChange = vi.fn();
    const onContextChange = vi.fn();
    const { trigger } = renderPicker({
      selectedModel: "opus",
      selectedReasoning: "high",
      selectedContext: "1m",
      reasoningOptions: CLAUDE_REASONING,
      contextOptions: CONTEXT_OPTIONS,
      onModelChange,
      onReasoningChange,
      onContextChange,
    });
    await userEvent.click(trigger);
    // Exactly one "Default" in the whole popover — the master row.
    expect(screen.getAllByText("Default")).toHaveLength(1);
    await userEvent.click(screen.getByText("Default"));
    expect(onModelChange).toHaveBeenCalledWith(null);
    expect(onReasoningChange).toHaveBeenCalledWith(null);
    expect(onContextChange).toHaveBeenCalledWith(null);
  });

  it("renders the reasoning section and reports the picked level", async () => {
    const onReasoningChange = vi.fn();
    const { trigger } = renderPicker({
      reasoningOptions: CLAUDE_REASONING,
      onReasoningChange,
    });
    await userEvent.click(trigger);
    expect(screen.getByText("Reasoning")).toBeInTheDocument();
    await userEvent.click(screen.getByText("Extra High"));
    expect(onReasoningChange).toHaveBeenCalledWith("xhigh");
  });

  it("renders the context section and reports the picked window", async () => {
    const onContextChange = vi.fn();
    const { trigger } = renderPicker({
      selectedModel: "sonnet",
      contextOptions: CONTEXT_OPTIONS,
      onContextChange,
    });
    await userEvent.click(trigger);
    expect(screen.getByText("Context Window")).toBeInTheDocument();
    await userEvent.click(screen.getByText("1M"));
    expect(onContextChange).toHaveBeenCalledWith("1m");
  });

  it("hides reasoning and context sections when no options are given", async () => {
    const { trigger } = renderPicker({ providerKind: "opencode" });
    await userEvent.click(trigger);
    expect(screen.queryByText("Reasoning")).not.toBeInTheDocument();
    expect(screen.queryByText("Context Window")).not.toBeInTheDocument();
  });
});

describe("LaunchModelPicker — selection marker", () => {
  it("marks exactly the selected row with data-checked and no others", async () => {
    const { trigger } = renderPicker({ selectedModel: "sonnet" });
    await userEvent.click(trigger);
    const checked = document.querySelectorAll(
      '[data-slot="command-item"][data-checked="true"]',
    );
    expect(checked).toHaveLength(1);
    expect(checked[0].textContent).toContain("Claude Sonnet");
  });

  it("checks the 'Default' row when no model is selected", async () => {
    const { trigger } = renderPicker();
    await userEvent.click(trigger);
    const checked = document.querySelectorAll(
      '[data-slot="command-item"][data-checked="true"]',
    );
    expect(checked).toHaveLength(1);
    expect(checked[0].textContent).toContain("Default");
  });
});

describe("LaunchModelPicker — adaptive search", () => {
  it("hides the search box for a short list", async () => {
    const { trigger } = renderPicker();
    await userEvent.click(trigger);
    expect(document.querySelector('[cmdk-input=""]')).not.toBeInTheDocument();
  });

  it("shows the search box once the list is long", async () => {
    const { trigger } = renderPicker({
      providerKind: "opencode",
      models: manyModels(24),
    });
    await userEvent.click(trigger);
    expect(document.querySelector('[cmdk-input=""]')).toBeInTheDocument();
  });
});
