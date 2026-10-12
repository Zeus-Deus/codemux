/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

vi.mock("./window-chrome", () => ({ WindowChrome: () => null }));

import { BootSplash } from "./boot-splash";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function wordmarkDelay(): string {
  return screen.getByTestId("boot-wordmark").style.animationDelay;
}

it("keeps the pulse phase fixed while the same splash re-renders", () => {
  const now = vi.spyOn(performance, "now").mockReturnValue(4300);
  const { rerender } = render(<BootSplash />);
  expect(wordmarkDelay()).toBe("-300ms");

  // App state, settings and flags arriving re-render the same instance.
  now.mockReturnValue(5150);
  rerender(<BootSplash />);
  expect(wordmarkDelay()).toBe("-300ms");
});

it("joins the running pulse when a later stage mounts its own copy", () => {
  vi.spyOn(performance, "now").mockReturnValue(5150);
  render(<BootSplash />);
  expect(wordmarkDelay()).toBe("-1150ms");
});

it("paints the wordmark in the theme colour at the static splash's size", () => {
  render(<BootSplash />);
  const wordmark = screen.getByTestId("boot-wordmark");
  // An <img> of the asset would keep its baked-in light fill on light themes.
  expect(wordmark.tagName.toLowerCase()).toBe("svg");
  expect(wordmark).toHaveClass("text-foreground");
  const fills = [...wordmark.querySelectorAll("[fill]")].map((el) =>
    el.getAttribute("fill"),
  );
  expect(fills.length).toBeGreaterThan(0);
  expect(new Set(fills)).toEqual(new Set(["currentColor"]));
  expect(wordmark.querySelectorAll("path")).toHaveLength(7);
  // Same box and viewBox as #splash in index.html, so the crossfade between
  // the two does not resize the logo.
  expect(wordmark.getAttribute("width")).toBe("320");
  expect(wordmark.getAttribute("height")).toBe("69");
  expect(wordmark.getAttribute("viewBox")).toBe("78 18 204 44");
});
