/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

import { geometryFingerprint } from "@/lib/geometry-fingerprint";

import { PanelHeader } from "./panel-header";

afterEach(() => cleanup());

describe("PanelHeader", () => {
  it("ships two documented heights, not one", () => {
    render(
      <>
        <PanelHeader variant="floating" data-testid="floating" />
        <PanelHeader variant="inline" data-testid="inline" />
      </>,
    );
    const floating = screen.getByTestId("floating");
    const inline = screen.getByTestId("inline");

    // 40px of GUI chrome that overlaps content, 36px of in-flow bar.
    expect(floating).toHaveClass("h-10");
    expect(inline).toHaveClass("h-9");
    expect(geometryFingerprint(floating)).not.toBe(
      geometryFingerprint(inline),
    );
  });

  it("rules off the in-flow variant and leaves the floating one clean", () => {
    render(
      <>
        <PanelHeader variant="floating" data-testid="floating" />
        <PanelHeader variant="inline" data-testid="inline" />
      </>,
    );
    // The floating band reaches the window edge; a rule there would draw a
    // slab across the titlebar. A call site that wants the seam asks.
    expect(screen.getByTestId("floating").className).not.toMatch(/border-b/);
    expect(screen.getByTestId("inline")).toHaveClass("border-b");
  });

  it("gives every adopting header the same padding, gap and slot", () => {
    render(
      <>
        <PanelHeader variant="floating" data-testid="floating" />
        <PanelHeader variant="inline" data-testid="inline" />
      </>,
    );
    for (const id of ["floating", "inline"]) {
      const header = screen.getByTestId(id);
      expect(header).toHaveAttribute("data-slot", "panel-header");
      expect(header).toHaveClass("items-center", "gap-2", "px-2.5");
    }
  });

  it("defaults to inline, the in-flow case", () => {
    render(<PanelHeader data-testid="header" />);
    expect(screen.getByTestId("header")).toHaveAttribute(
      "data-variant",
      "inline",
    );
    expect(screen.getByTestId("header")).toHaveClass("h-9");
  });

  it("lets a caller override an inset without losing the height", () => {
    // Dense pane chrome (the browser toolbar) needs tighter padding; it
    // must not have to re-declare the height to get it.
    render(<PanelHeader className="px-1" data-testid="header" />);
    const header = screen.getByTestId("header");
    expect(header).toHaveClass("px-1", "h-9");
    expect(header).not.toHaveClass("px-2.5");
  });

  it("renders a header landmark, or delegates with asChild", () => {
    const { container } = render(
      <>
        <PanelHeader data-testid="own" />
        <PanelHeader asChild>
          <div data-testid="delegated" />
        </PanelHeader>
      </>,
    );
    expect(container.querySelector("header")).not.toBeNull();
    expect(screen.getByTestId("delegated").tagName).toBe("DIV");
    expect(screen.getByTestId("delegated")).toHaveClass("h-9");
  });
});
