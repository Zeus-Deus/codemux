/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";

import type { AgentChatProviderKind } from "@/tauri/types";

import { AssistantAvatar } from "./AssistantAvatar";

// Vite serves resolved `?import` URLs for SVGs in dev/prod; vitest's
// jsdom env doesn't. Stub the three provider marks so the component gets
// back a predictable path string we can assert on (same pattern as
// provider-logo.test.tsx / ModelPicker.test.tsx).
vi.mock("@/assets/preset-icons/claude.svg", () => ({
  default: "/mock/claude.svg",
}));
vi.mock("@/assets/preset-icons/codex.svg", () => ({
  default: "/mock/codex.svg",
}));
vi.mock("@/assets/preset-icons/opencode.svg", () => ({
  default: "/mock/opencode.svg",
}));

afterEach(() => cleanup());

const PROVIDERS: AgentChatProviderKind[] = ["claude", "codex", "opencode"];

describe("AssistantAvatar", () => {
  it.each(PROVIDERS)(
    "renders the %s provider mark with data-provider",
    (provider) => {
      const { container } = render(<AssistantAvatar provider={provider} />);
      const img = container.querySelector("img") as HTMLImageElement;
      expect(img).not.toBeNull();
      expect(img.getAttribute("data-provider")).toBe(provider);
      expect(img.getAttribute("src")).toContain(`${provider}.svg`);
      // No sparkle fallback when a provider is present.
      expect(container.querySelector("svg")).toBeNull();
    },
  );

  it("keeps the ember wash for the ember-toned Claude mark", () => {
    const { container } = render(<AssistantAvatar provider="claude" />);
    const box = container.querySelector("[data-provider]") as HTMLElement;
    expect(box.className).toContain("bg-accent-ember/15");
    expect(box.className).not.toContain("bg-foreground/8");
  });

  it.each(["codex", "opencode"] as const)(
    "uses a neutral wash for the neutral %s mark",
    (provider) => {
      const { container } = render(<AssistantAvatar provider={provider} />);
      const box = container.querySelector("[data-provider]") as HTMLElement;
      expect(box.className).toContain("bg-foreground/8");
      expect(box.className).not.toContain("bg-accent-ember");
    },
  );

  it("falls back to the sparkle when no provider is given", () => {
    const { container } = render(<AssistantAvatar />);
    // The fallback is the lucide sparkle (an inline svg), not an <img>.
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("svg")).not.toBeNull();
    // Fallback keeps the original ember tint + text color.
    const box = container.firstElementChild as HTMLElement;
    expect(box.className).toContain("bg-accent-ember/15");
    expect(box.className).toContain("text-accent-ember");
  });

  it("falls back to the sparkle when provider is explicitly null", () => {
    const { container } = render(<AssistantAvatar provider={null} />);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("svg")).not.toBeNull();
  });
});
