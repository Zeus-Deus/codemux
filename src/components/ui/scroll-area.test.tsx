/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

import { ScrollArea } from "./scroll-area";

afterEach(() => cleanup());

describe("ScrollArea", () => {
  it("does not rewrite Radix's inline style when it re-renders", () => {
    // WebKitGTK re-parses a rewritten <style> and recalculates the whole page.
    const { container, rerender } = render(<ScrollArea><p>one</p></ScrollArea>);
    const style = container.querySelector("[data-slot=scroll-area] style")!;
    const text = style.firstChild;
    rerender(<ScrollArea><p>two</p></ScrollArea>);
    expect(screen.getByText("two")).toBeInTheDocument();
    expect(style.firstChild).toBe(text);
  });

  it("clips its own box so positioned descendants cannot grow the page", () => {
    // Radix's viewport is not positioned, so sr-only labels and other
    // absolute children resolve against the root. Unclipped, they made the
    // whole window scroll past the app at the bottom of long Settings pages.
    const { container } = render(<ScrollArea><p>content</p></ScrollArea>);
    expect(container.querySelector("[data-slot=scroll-area]")).toHaveClass("relative", "overflow-hidden");
  });

  it("still applies style changes", () => {
    const style = document.createElement("style");
    style.innerHTML = "a{color:red}";
    const text = style.firstChild;
    style.innerHTML = "a{color:red}";
    expect(style.firstChild).toBe(text);
    style.innerHTML = "a{color:blue}";
    expect(style.innerHTML).toBe("a{color:blue}");
  });
});
