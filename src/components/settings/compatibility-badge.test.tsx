/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

import { CompatibilityBadge } from "./compatibility-badge";

afterEach(() => cleanup());

describe("CompatibilityBadge", () => {
  it("renders 'Tool refs' label and amber styling for soft-warn", () => {
    render(<CompatibilityBadge level="soft-warn" />);
    const badge = screen.getByTestId("compatibility-badge");
    expect(badge).toHaveTextContent("Tool refs");
    expect(badge).toHaveAttribute("data-level", "soft-warn");
    expect(badge.className).toContain("text-amber-600");
  });

  it("renders 'May not work' label and destructive styling for hard-warn", () => {
    render(<CompatibilityBadge level="hard-warn" />);
    const badge = screen.getByTestId("compatibility-badge");
    expect(badge).toHaveTextContent("May not work");
    expect(badge).toHaveAttribute("data-level", "hard-warn");
    expect(badge.className).toContain("text-destructive");
  });
});
