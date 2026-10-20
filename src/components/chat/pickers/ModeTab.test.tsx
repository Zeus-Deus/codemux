/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

import { ModeTab, MODE_CONFIG } from "./ModeTab";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("ModeTab", () => {
  it.each([
    ["plan", "Plan"],
    ["ask", "Ask"],
    ["debug", "Debug"],
  ] as const)("renders the %s tab tinted from MODE_CONFIG", (mode, label) => {
    render(<ModeTab mode={mode} onRemove={() => {}} />);
    const tab = screen.getByRole("status", { name: `${label} mode active` });
    expect(tab).toHaveTextContent(label);
    expect(tab).toHaveClass(
      MODE_CONFIG[mode].bg,
      MODE_CONFIG[mode].text,
      MODE_CONFIG[mode].border,
    );
    // Flush on the card's top edge: no bottom border.
    expect(tab).toHaveClass("border-b-0");
  });

  it("renders nothing in Build mode", () => {
    const { container } = render(<ModeTab mode={null} onRemove={() => {}} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("the leading icon is the leave button, named after the mode", () => {
    const onRemove = vi.fn();
    render(<ModeTab mode="plan" onRemove={onRemove} />);
    const leave = screen.getByRole("button", { name: "Leave Plan mode" });
    // Icon and × share one grid cell, so the swap can't resize the tab.
    const [icon, x] = Array.from(leave.querySelectorAll("svg"));
    expect(icon).toHaveClass("col-start-1", "row-start-1", "group-hover/tab:opacity-0");
    expect(x).toHaveClass("col-start-1", "row-start-1", "opacity-0", "group-hover/tab:opacity-100");
    // Touch has no hover, so the × is always there.
    expect(x).toHaveClass("pointer-coarse:opacity-100", "group-focus-visible/leave:opacity-100");
    fireEvent.click(leave);
    expect(onRemove).toHaveBeenCalledTimes(1);
  });

  it("holds the last mode through the exit slide, then unmounts", () => {
    vi.useFakeTimers();
    const { rerender, container } = render(
      <ModeTab mode="ask" onRemove={() => {}} />,
    );
    rerender(<ModeTab mode={null} onRemove={() => {}} />);
    const slot = screen.getByTestId("composer-mode-tab");
    expect(slot).toHaveAttribute("data-leaving");
    expect(slot).toHaveAttribute("aria-hidden", "true");
    expect(slot).toHaveTextContent("Ask");
    expect(slot.firstElementChild).toHaveClass("slide-out-to-bottom");

    fireEvent.animationEnd(slot.firstElementChild!);
    expect(container).toBeEmptyDOMElement();
  });

  it("unmounts on the backstop when animationend never arrives", () => {
    vi.useFakeTimers();
    const { rerender, container } = render(
      <ModeTab mode="debug" onRemove={() => {}} />,
    );
    rerender(<ModeTab mode={null} onRemove={() => {}} />);
    expect(container).not.toBeEmptyDOMElement();
    act(() => vi.advanceTimersByTime(250));
    expect(container).toBeEmptyDOMElement();
  });

  it("re-entering mid-exit slides back up with the new mode", () => {
    const { rerender } = render(<ModeTab mode="plan" onRemove={() => {}} />);
    rerender(<ModeTab mode={null} onRemove={() => {}} />);
    rerender(<ModeTab mode="debug" onRemove={() => {}} />);
    const tab = screen.getByRole("status", { name: "Debug mode active" });
    expect(tab).toHaveClass("slide-in-from-bottom");
    expect(screen.getByTestId("composer-mode-tab")).not.toHaveAttribute(
      "data-leaving",
    );
  });
});
