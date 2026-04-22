/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import { ChatHomeLanding } from "./ChatHomeLanding";

describe("ChatHomeLanding", () => {
  it("renders the marquee headline", () => {
    const { container } = render(
      <ChatHomeLanding composer={<div data-testid="composer-slot" />} />,
    );
    const heading = container.querySelector("h1");
    expect(heading?.textContent).toBe("What should we do today?");
  });

  it("renders the composer passed via props", () => {
    const { container } = render(
      <ChatHomeLanding
        composer={<div data-testid="composer-slot-a">slotA</div>}
      />,
    );
    expect(
      container.querySelector('[data-testid="composer-slot-a"]'),
    ).not.toBeNull();
  });

  it("uses only neutral foreground/muted color tokens — no accents", () => {
    const { container } = render(
      <ChatHomeLanding composer={<div />} />,
    );
    const html = container.innerHTML;
    expect(html).not.toMatch(/\btext-primary\b/);
    expect(html).not.toMatch(/\bbg-primary\b/);
    expect(html).not.toMatch(/\btext-success\b/);
    expect(html).not.toMatch(/\btext-warning\b/);
    expect(html).not.toMatch(/\btext-danger\b/);
  });

  it("centers content vertically and horizontally", () => {
    const { container } = render(
      <ChatHomeLanding composer={<div />} />,
    );
    const root = container.firstElementChild as HTMLElement;
    expect(root.className).toContain("items-center");
    expect(root.className).toContain("justify-center");
  });
});
