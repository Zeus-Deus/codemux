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
  const img = screen.getByRole("status").querySelector("img");
  return img?.style.animationDelay ?? "";
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
