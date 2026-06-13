/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { LaunchReasoningPicker } from "./launch-reasoning-picker";

const REASONING = [
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
  { value: "xhigh", label: "Extra High" },
  { value: "max", label: "Max" },
];

const CONTEXT = [
  { value: "200k", label: "200k" },
  { value: "1m", label: "1M" },
];

type PickerProps = Parameters<typeof LaunchReasoningPicker>[0];

function renderPicker(overrides: Partial<PickerProps> = {}) {
  const props: PickerProps = {
    reasoningOptions: REASONING,
    selectedReasoning: null,
    defaultReasoning: "high",
    onReasoningChange: vi.fn(),
    contextOptions: CONTEXT,
    selectedContext: null,
    defaultContext: "1m",
    onContextChange: vi.fn(),
    ...overrides,
  };
  const result = render(<LaunchReasoningPicker {...props} />);
  const trigger = result.container.querySelector("button");
  return { ...result, props, trigger };
}

afterEach(() => cleanup());

describe("LaunchReasoningPicker — visibility", () => {
  it("hides entirely when the model has neither reasoning nor context (e.g. Haiku)", () => {
    const { trigger } = renderPicker({
      reasoningOptions: [],
      contextOptions: [],
    });
    expect(trigger).toBeNull();
  });

  it("renders when the model has reasoning options only", () => {
    const { trigger } = renderPicker({ contextOptions: [] });
    expect(trigger).not.toBeNull();
  });
});

describe("LaunchReasoningPicker — pill label", () => {
  it("falls back to the model defaults ('High · 1M') when nothing is picked", () => {
    const { trigger } = renderPicker();
    expect(trigger?.textContent).toContain("High");
    expect(trigger?.textContent).toContain("1M");
  });

  it("reflects explicit picks over the defaults", () => {
    const { trigger } = renderPicker({
      selectedReasoning: "max",
      selectedContext: "200k",
    });
    expect(trigger?.textContent).toContain("Max");
    expect(trigger?.textContent).toContain("200k");
  });
});

describe("LaunchReasoningPicker — interaction", () => {
  it("shows both sections and reports the picked reasoning level", async () => {
    const onReasoningChange = vi.fn();
    const { trigger } = renderPicker({ onReasoningChange });
    await userEvent.click(trigger as HTMLElement);
    expect(screen.getByText("Reasoning")).toBeInTheDocument();
    expect(screen.getByText("Context Window")).toBeInTheDocument();
    await userEvent.click(screen.getByText("Extra High"));
    expect(onReasoningChange).toHaveBeenCalledWith("xhigh");
  });

  it("reports the picked context window", async () => {
    const onContextChange = vi.fn();
    const { trigger } = renderPicker({ onContextChange });
    await userEvent.click(trigger as HTMLElement);
    await userEvent.click(screen.getByText("200k"));
    expect(onContextChange).toHaveBeenCalledWith("200k");
  });

  it("omits the Context Window section when the model has no context options", async () => {
    const { trigger } = renderPicker({ contextOptions: [] });
    await userEvent.click(trigger as HTMLElement);
    expect(screen.getByText("Reasoning")).toBeInTheDocument();
    expect(screen.queryByText("Context Window")).not.toBeInTheDocument();
  });
});
