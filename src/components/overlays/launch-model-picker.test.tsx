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
    onModelChange: vi.fn(),
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

  it("shows the model label (and no reasoning/context suffix — those live in the sibling pill)", () => {
    const { trigger } = renderPicker({ selectedModel: "sonnet" });
    expect(trigger.textContent).toContain("Claude Sonnet");
    // The model pill carries the model only; reasoning/context are a
    // separate pill now.
    expect(trigger.textContent).not.toContain("·");
  });
});

describe("LaunchModelPicker — interaction", () => {
  it("opens the popover and reports the picked model, then closes", async () => {
    const onModelChange = vi.fn();
    const { trigger } = renderPicker({ onModelChange });
    await userEvent.click(trigger);
    await userEvent.click(screen.getByText("Claude Opus"));
    expect(onModelChange).toHaveBeenCalledWith("opus");
    // Popover closes on pick — the model list is gone.
    expect(screen.queryByText("Claude Sonnet")).not.toBeInTheDocument();
  });

  it("has a single 'Default' row that resets the model to null", async () => {
    const onModelChange = vi.fn();
    const { trigger } = renderPicker({ selectedModel: "opus", onModelChange });
    await userEvent.click(trigger);
    // Exactly one "Default" in the whole popover — the master row.
    expect(screen.getAllByText("Default")).toHaveLength(1);
    await userEvent.click(screen.getByText("Default"));
    expect(onModelChange).toHaveBeenCalledWith(null);
  });

  it("no longer renders reasoning or context sections (moved to LaunchReasoningPicker)", async () => {
    const { trigger } = renderPicker({ selectedModel: "sonnet" });
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

  it("focuses the search box on open so typing filters without a click", async () => {
    const { trigger } = renderPicker({
      providerKind: "opencode",
      models: manyModels(24),
    });
    await userEvent.click(trigger);
    const input = document.querySelector('[cmdk-input=""]') as HTMLElement;
    expect(input).toHaveFocus();
    // Deliberately NO click on the input — keystrokes must land in it
    // straight from the popover's open-autofocus.
    await userEvent.keyboard("Model 3");
    expect(input).toHaveValue("Model 3");
    expect(screen.getByText("Model 3")).toBeInTheDocument();
    expect(screen.queryByText("Model 1")).not.toBeInTheDocument();
  });
});
