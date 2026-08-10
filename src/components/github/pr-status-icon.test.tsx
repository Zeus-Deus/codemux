/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, afterEach } from "vitest";
import { render, cleanup } from "@testing-library/react";
import {
  PrStatusIcon,
  prStatusRecededCardHoverClass,
  prStatusSettledHoverClass,
} from "./pr-status-icon";

afterEach(cleanup);

describe("PrStatusIcon", () => {
  it("colors the glyph by PR state", () => {
    const { container } = render(<PrStatusIcon state="OPEN" />);
    const svg = container.querySelector("svg") as SVGElement;
    expect(svg.getAttribute("class")).toContain("text-status-open");
  });

  it("lets a caller's color win, so the sidebar's settled badge can hand the glyph currentColor", () => {
    // The settled row paints the whole badge (icon + "#n") from one class on
    // the button and passes `text-current` down, so number and glyph light up
    // together on hover. That only works because the built-in state color is
    // merged *before* the caller's className — tailwind-merge resolves the
    // conflict caller-wins. Reverse the `cn()` argument order inside
    // `PrStatusIcon` and the glyph silently keeps its own color.
    const { container } = render(
      <PrStatusIcon state="OPEN" className="text-current" />,
    );
    const cls = container.querySelector("svg")?.getAttribute("class") ?? "";
    expect(cls).toContain("text-current");
    expect(cls).not.toMatch(/(^|\s)text-status-open\b/);
  });

  it("defers each state's color to the settled row's hover/focus variants", () => {
    // Same palette as the resting maps, only later — the settled shelf greys
    // out at rest and repaints under the pointer.
    expect(prStatusSettledHoverClass("MERGED")).toContain(
      "group-hover/settled:text-accent-violet",
    );
    expect(prStatusSettledHoverClass("open")).toContain(
      "group-focus-within/settled:text-status-open",
    );
    expect(prStatusSettledHoverClass("closed")).toContain(
      "group-hover/settled:text-destructive",
    );
    expect(prStatusSettledHoverClass("draft")).toContain(
      "group-hover/settled:text-muted-foreground",
    );
    expect(prStatusSettledHoverClass("weird")).toBeNull();
    expect(prStatusSettledHoverClass(null)).toBeNull();
  });

  it("defers each state's color to a receded card's hover/focus variants", () => {
    expect(prStatusRecededCardHoverClass("MERGED")).toContain(
      "group-hover/card:text-accent-violet",
    );
    expect(prStatusRecededCardHoverClass("open")).toContain(
      "group-focus-within/card:text-status-open",
    );
    expect(prStatusRecededCardHoverClass("closed")).toContain(
      "group-hover/card:text-destructive",
    );
    expect(prStatusRecededCardHoverClass("draft")).toContain(
      "group-hover/card:text-muted-foreground",
    );
    expect(prStatusRecededCardHoverClass("weird")).toBeNull();
    expect(prStatusRecededCardHoverClass(null)).toBeNull();
  });
});
