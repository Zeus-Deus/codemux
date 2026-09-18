import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { mobileLayoutAvailable, useMobileViewport } from "./use-mobile-layout";

beforeEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  delete (window as unknown as Record<string, unknown>).__CODEMUX_REMOTE__;
  delete document.documentElement.dataset.browserClient;
  vi.stubGlobal("matchMedia", () => ({
    matches: true,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  }));
});
afterEach(() => vi.unstubAllGlobals());
it("keeps a narrow native desktop window on desktop UI", () => {
  vi.stubGlobal("__TAURI_INTERNALS__", {});
  expect(mobileLayoutAvailable()).toBe(false);
  vi.stubGlobal("__CODEMUX_REMOTE__", true);
  expect(mobileLayoutAvailable()).toBe(true);
});
it("tracks the keyboard viewport, ignores pinch zoom, and cleans up on unmount", () => {
  const viewport = Object.assign(new EventTarget(), {
    height: 844,
    offsetTop: 0,
    scale: 1,
  });
  vi.stubGlobal("visualViewport", viewport);
  vi.stubGlobal("innerHeight", 844);
  const root = document.documentElement;
  const { unmount } = renderHook(useMobileViewport);
  expect(root.style.getPropertyValue("--mobile-height")).toBe("844px");
  act(() => {
    viewport.height = 450;
    viewport.offsetTop = 30;
    viewport.dispatchEvent(new Event("resize"));
  });
  expect(root.dataset.keyboard).toBe("true");
  expect(root.style.getPropertyValue("--mobile-height")).toBe("450px");
  expect(root.style.getPropertyValue("--mobile-top")).toBe("30px");
  act(() => {
    viewport.scale = 2;
    viewport.height = 200;
    viewport.dispatchEvent(new Event("resize"));
  });
  expect(root.style.getPropertyValue("--mobile-height")).toBe("450px");
  unmount();
  expect(root.dataset.mobile).toBeUndefined();
  expect(root.style.getPropertyValue("--mobile-height")).toBe("");
});
