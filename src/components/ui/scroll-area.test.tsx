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
