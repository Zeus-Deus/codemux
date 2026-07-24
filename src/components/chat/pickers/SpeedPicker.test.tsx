/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { ChatModelInfo } from "@/tauri/types";

import { SpeedPicker } from "./SpeedPicker";

afterEach(() => cleanup());

const MODEL: ChatModelInfo = {
  id: "gpt-5.6",
  label: "GPT-5.6",
  description: null,
  effort_levels: ["medium", "high"],
  default_effort: "medium",
  prompt_injected_effort_levels: [],
  context_window_options: [],
  supports_adaptive_thinking: false,
  supports_thinking_toggle: false,
  supports_fast_mode: true,
  supports_images: true,
  sub_provider: null,
  is_free: false,
};

describe("SpeedPicker", () => {
  it("hides unless the active model advertises fast mode", () => {
    const { container } = render(
      <SpeedPicker
        model={{ ...MODEL, supports_fast_mode: false }}
        value={false}
        onChange={vi.fn()}
      />,
    );
    expect(container.querySelector("button")).toBeNull();
  });

  it("shows the current speed and selects Fast explicitly", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<SpeedPicker model={MODEL} value={false} onChange={onChange} />);

    await user.click(screen.getByRole("button", { name: "Speed: Standard" }));
    await user.click(screen.getByText("Fast", { selector: "span" }));

    expect(onChange).toHaveBeenCalledWith(true);
  });

  it("uses an ember active treatment when Fast is selected", () => {
    render(<SpeedPicker model={MODEL} value onChange={vi.fn()} />);
    const trigger = screen.getByRole("button", { name: "Speed: Fast" });
    expect(trigger).toHaveClass("text-accent-ember");
  });
});
