/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import { ModePill, MODE_CONFIG } from "./ModePill";

afterEach(() => cleanup());

describe("ModePill", () => {
  it("renders the Plan variant with --primary tint and label", () => {
    const { container } = render(
      <ModePill mode="plan" onRemove={() => {}} />,
    );
    expect(screen.getByText("Plan")).toBeInTheDocument();
    // Color token comes from the MODE_CONFIG table.
    expect(container.innerHTML).toContain(MODE_CONFIG.plan.bg);
    expect(container.innerHTML).toContain(MODE_CONFIG.plan.text);
    // Status role wired so screen readers announce the pill.
    expect(screen.getByRole("status")).toHaveAttribute(
      "aria-label",
      "Plan mode active",
    );
  });

  it("renders the Ask variant with --success tint", () => {
    const { container } = render(
      <ModePill mode="ask" onRemove={() => {}} />,
    );
    expect(screen.getByText("Ask")).toBeInTheDocument();
    expect(container.innerHTML).toContain(MODE_CONFIG.ask.bg);
    expect(container.innerHTML).toContain(MODE_CONFIG.ask.text);
  });

  it("renders the Debug variant with --danger tint", () => {
    const { container } = render(
      <ModePill mode="debug" onRemove={() => {}} />,
    );
    expect(screen.getByText("Debug")).toBeInTheDocument();
    expect(container.innerHTML).toContain(MODE_CONFIG.debug.bg);
    expect(container.innerHTML).toContain(MODE_CONFIG.debug.text);
  });

  it("X button fires onRemove and does not bubble to the pill onClick", () => {
    const onRemove = vi.fn();
    const onClick = vi.fn();
    render(
      <ModePill mode="plan" onRemove={onRemove} onClick={onClick} />,
    );
    fireEvent.click(
      screen.getByRole("button", { name: /Remove Plan mode/i }),
    );
    expect(onRemove).toHaveBeenCalledTimes(1);
    expect(onClick).not.toHaveBeenCalled();
  });

  it("clicking the pill body fires onClick when supplied", () => {
    const onClick = vi.fn();
    render(
      <ModePill mode="plan" onRemove={() => {}} onClick={onClick} />,
    );
    fireEvent.click(screen.getByRole("status"));
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
