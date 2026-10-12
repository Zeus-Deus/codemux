/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, waitFor } from "@testing-library/react";
import { toast } from "sonner";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const scheme = vi.hoisted(() => ({ current: "light" as "light" | "dark" }));
vi.mock("@/hooks/use-app-theme", () => ({
  useAppTheme: () => ({ theme: { scheme: scheme.current } }),
}));

import { Toaster } from "./sonner";

afterEach(() => {
  act(() => toast.dismiss());
  cleanup();
});

async function renderedTheme(): Promise<string | null> {
  render(<Toaster />);
  act(() => {
    toast("Pushed", { description: "Tap Undo within 10s" });
  });
  // Sonner commits new toasts on a timeout, and only then renders its list.
  const list = await waitFor(() => {
    const el = document.querySelector("[data-sonner-toaster]");
    expect(el).not.toBeNull();
    return el;
  });
  return list?.getAttribute("data-sonner-theme") ?? null;
}

describe("Toaster", () => {
  // Sonner's stylesheet picks description and close-button greys from this
  // attribute; a dark toaster on a light theme paints near-white text on white.
  it("follows a light theme", async () => {
    scheme.current = "light";
    expect(await renderedTheme()).toBe("light");
  });

  it("follows a dark theme", async () => {
    scheme.current = "dark";
    expect(await renderedTheme()).toBe("dark");
  });

  // Sonner pauses its dismiss timer while the document is hidden; the class
  // lets the undo bar pause with it.
  it("marks the toaster while the window is hidden", async () => {
    const hidden = vi.spyOn(document, "hidden", "get").mockReturnValue(false);
    try {
      await renderedTheme();
      const list = () => document.querySelector("[data-sonner-toaster]");
      expect(list()).not.toHaveClass("cm-toaster-hidden");
      hidden.mockReturnValue(true);
      act(() => {
        document.dispatchEvent(new Event("visibilitychange"));
      });
      expect(list()).toHaveClass("cm-toaster-hidden");
      hidden.mockReturnValue(false);
      act(() => {
        document.dispatchEvent(new Event("visibilitychange"));
      });
      expect(list()).not.toHaveClass("cm-toaster-hidden");
    } finally {
      hidden.mockRestore();
    }
  });
});

describe("undo bar CSS", () => {
  // The drain rule uses the `animation` shorthand, which resets
  // animation-play-state to running. Each pause selector only wins if it
  // carries every qualifier of the drain rule and more.
  it("pauses with selectors that out-rank the drain rule", () => {
    const css = readFileSync(resolve(process.cwd(), "src/globals.css"), "utf8").replace(
      /\/\*[\s\S]*?\*\//g,
      "",
    );
    const selectorBefore = (declaration: string) =>
      css.match(new RegExp(`([^{}]+)\\{[^{}]*${declaration}`))?.[1].trim() ?? "";
    const tokens = (selector: string): string[] => selector.match(/\[[^\]]+\]|\.[\w-]+/g) ?? [];
    const drain = tokens(selectorBefore("animation: cm-toast-undo-drain"));
    const pauses = selectorBefore("animation-play-state: paused").split(",").map(tokens);
    expect(drain.length).toBeGreaterThan(0);
    for (const pause of pauses) {
      expect(pause).toEqual(expect.arrayContaining(drain));
      expect(pause.length).toBeGreaterThan(drain.length);
    }
    expect(pauses.map((pause) => pause.find((token) => !drain.includes(token)))).toEqual([
      '[data-expanded="true"]',
      ".cm-toaster-hidden",
    ]);
  });
});
