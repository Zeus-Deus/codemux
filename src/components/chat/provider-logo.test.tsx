/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi } from "vitest";
import { render } from "@testing-library/react";
import { ProviderLogo } from "./provider-logo";

// Vite serves `?import` URLs for SVGs in dev mode; vitest's jsdom env
// doesn't. Stub the two asset modules so the component gets back a
// predictable path string we can assert on. The prod build uses the
// real asset-url loader via Vite.
vi.mock("@/assets/preset-icons/claude.svg", () => ({
  default: "/mock/claude.svg",
}));
vi.mock("@/assets/preset-icons/codex.svg", () => ({
  default: "/mock/codex.svg",
}));
vi.mock("@/assets/preset-icons/grok.svg", () => ({
  default: "/mock/grok.svg",
}));

describe("ProviderLogo", () => {
  it("renders the Claude icon for provider='claude'", () => {
    const { container } = render(<ProviderLogo provider="claude" />);
    const img = container.querySelector("img") as HTMLImageElement;
    expect(img).not.toBeNull();
    expect(img.getAttribute("data-provider")).toBe("claude");
    expect(img.getAttribute("src")).toContain("claude.svg");
    expect(img.getAttribute("alt")).toBe("Claude");
  });

  it("renders the Codex icon for provider='codex'", () => {
    const { container } = render(<ProviderLogo provider="codex" />);
    const img = container.querySelector("img") as HTMLImageElement;
    expect(img).not.toBeNull();
    expect(img.getAttribute("data-provider")).toBe("codex");
    expect(img.getAttribute("src")).toContain("codex.svg");
    expect(img.getAttribute("alt")).toBe("Codex");
  });

  it("renders the Grok icon and accessible label", () => {
    const { container } = render(<ProviderLogo provider="grok" />);
    const img = container.querySelector("img") as HTMLImageElement;
    expect(img.getAttribute("data-provider")).toBe("grok");
    expect(img.getAttribute("src")).toContain("grok.svg");
    expect(img.getAttribute("alt")).toBe("Grok");
  });

  it("forwards className for sizing", () => {
    const { container } = render(
      <ProviderLogo provider="claude" className="h-5 w-5" />,
    );
    const img = container.querySelector("img") as HTMLImageElement;
    expect(img.className).toContain("h-5");
    expect(img.className).toContain("w-5");
  });
});
