/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { EdgeFades, useScrollEdges } from "./scroll-fades";

afterEach(cleanup);

function Scroller() {
  const [ref, edges] = useScrollEdges<HTMLDivElement>();
  return (
    <div>
      <div ref={ref} data-testid="scroller" />
      <EdgeFades edges={edges} className="from-popover" testId="list" />
    </div>
  );
}

/** jsdom has no layout: give the scroller 300px of content in a 100px box. */
function layout(node: HTMLElement, scrollTop: number) {
  Object.defineProperty(node, "scrollHeight", { configurable: true, value: 300 });
  Object.defineProperty(node, "clientHeight", { configurable: true, value: 100 });
  node.scrollTop = scrollTop;
  fireEvent.scroll(node);
}

it("shows a fade only on the edges with more to scroll", () => {
  render(<Scroller />);
  const top = screen.getByTestId("list-top-fade");
  const bottom = screen.getByTestId("list-bottom-fade");
  // Nothing overflows yet.
  expect(top).toHaveAttribute("data-visible", "false");
  expect(bottom).toHaveAttribute("data-visible", "false");

  const scroller = screen.getByTestId("scroller");
  layout(scroller, 0);
  expect(top).toHaveAttribute("data-visible", "false");
  expect(bottom).toHaveAttribute("data-visible", "true");

  layout(scroller, 100);
  expect(top).toHaveAttribute("data-visible", "true");
  expect(bottom).toHaveAttribute("data-visible", "true");
  expect(top).toHaveClass("opacity-100", "duration-150");

  layout(scroller, 200);
  expect(top).toHaveAttribute("data-visible", "true");
  expect(bottom).toHaveAttribute("data-visible", "false");
  expect(bottom).toHaveClass("opacity-0");
});
