import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "./tooltip";

afterEach(cleanup);

function renderTrigger() {
  render(
    <TooltipProvider delayDuration={0}>
      <button type="button">Open details</button>
      <Tooltip>
        <TooltipTrigger>Run</TooltipTrigger>
        <TooltipContent>Set a run command</TooltipContent>
      </Tooltip>
    </TooltipProvider>,
  );
  return screen.getByRole("button", { name: "Run" });
}

describe("TooltipTrigger focus", () => {
  it("opens when focus arrives by Tab", () => {
    const trigger = renderTrigger();
    fireEvent.keyDown(document, { key: "Tab" });
    act(() => trigger.focus());
    expect(screen.queryByRole("tooltip")).not.toBeNull();
  });

  it("opens when focus arrives by arrow-key roving", () => {
    const trigger = renderTrigger();
    fireEvent.keyDown(document, { key: "ArrowDown" });
    act(() => trigger.focus());
    expect(screen.queryByRole("tooltip")).not.toBeNull();
  });

  it.each(["Enter", "o", "Escape"])(
    "stays shut when a popup focuses it after %s",
    (key) => {
      const trigger = renderTrigger();
      // Focus moves straight from the opener, as a popup's auto-focus does.
      act(() => screen.getByRole("button", { name: "Open details" }).focus());
      fireEvent.keyDown(document, { key });
      act(() => trigger.focus());
      expect(screen.queryByRole("tooltip")).toBeNull();
    },
  );

  it("stays shut when focus follows a pointer press", () => {
    const trigger = renderTrigger();
    fireEvent.keyDown(document, { key: "Tab" });
    fireEvent.pointerDown(document.body);
    act(() => trigger.focus());
    expect(screen.queryByRole("tooltip")).toBeNull();
  });
});
