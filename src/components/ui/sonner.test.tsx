/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, waitFor } from "@testing-library/react";
import { toast } from "sonner";

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
});
