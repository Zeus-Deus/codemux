/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, expect, it } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { EdgeFades, useScrollEdges } from "./scroll-fades";

afterEach(cleanup);

function Scroller() {
  const [ref, edges] = useScrollEdges<HTMLDivElement>();
  return (
    <div>
      <div ref={ref} data-testid="scroller">
        <span data-testid="label">GPT-5.4</span>
      </div>
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

it("follows content that changes without a scroll", async () => {
  render(<Scroller />);
  const scroller = screen.getByTestId("scroller");
  // A refreshed label wraps and the content outgrows the box; nothing scrolls.
  Object.defineProperty(scroller, "scrollHeight", { configurable: true, value: 300 });
  Object.defineProperty(scroller, "clientHeight", { configurable: true, value: 100 });
  act(() => {
    screen.getByTestId("label").firstChild!.textContent = "A much longer model name that wraps";
  });
  await waitFor(() => expect(screen.getByTestId("list-bottom-fade")).toHaveAttribute("data-visible", "true"));
});
