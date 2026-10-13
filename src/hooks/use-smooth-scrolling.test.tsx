import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";

vi.mock("@/lib/wheel-scrolling", () => ({ installWheelScrolling: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => false) }));
vi.mock("@/tauri/commands", () => ({
  dbGetAllSettings: vi.fn(async () => ({})),
  dbSetSetting: vi.fn(async () => undefined),
}));
import { installWheelScrolling } from "@/lib/wheel-scrolling";
import { invoke } from "@tauri-apps/api/core";
import { useSettingsStore } from "@/stores/settings-store";
import { useSmoothScrollingInit } from "./use-smooth-scrolling";

const install = vi.mocked(installWheelScrolling);
const stop = vi.fn();
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
beforeEach(() => {
  vi.clearAllMocks();
  install.mockReturnValue(stop);
  vi.mocked(invoke).mockResolvedValue(false);
  useSettingsStore.setState({ settings: {}, loaded: false });
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 Version/60.5 Safari/605.1.15",
  );
});

describe("default Linux wheel scrolling", () => {
  it("installs without waiting for settings or requiring an opt-in", () => {
    renderHook(() => useSmoothScrollingInit());
    expect(install).toHaveBeenCalledTimes(1);
  });
  it("leaves scrolling native when the app reduced-motion setting is enabled", () => {
    useSettingsStore.setState({ settings: { "appearance.reduce_motion": "true" } });
    renderHook(() => useSmoothScrollingInit());
    expect(install).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalled();
  });
  it("cancels an active glide when reduced motion is enabled and reinstalls when disabled", async () => {
    const wheel = await vi.importActual<typeof import("@/lib/wheel-scrolling")>("@/lib/wheel-scrolling");
    install.mockImplementation(wheel.installWheelScrolling);
    vi.stubGlobal("matchMedia", vi.fn(() => ({
      matches: false,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    })));
    const frames = new Map<number, FrameRequestCallback>();
    let frameId = 0;
    vi.spyOn(window.performance, "now").mockReturnValue(0);
    vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
      frames.set(++frameId, callback);
      return frameId;
    });
    vi.spyOn(window, "cancelAnimationFrame").mockImplementation((id) => {
      frames.delete(id);
    });
    const viewport = document.createElement("div");
    Object.defineProperties(viewport, {
      scrollHeight: { value: 1000 },
      clientHeight: { value: 100 },
    });
    document.body.append(viewport);
    const view = renderHook(() => useSmoothScrollingInit());
    try {
      expect(wheel.tryAnimatedWheelScroll(viewport, 120)).toBe(true);
      const callback = frames.get(frameId)!;
      frames.delete(frameId);
      callback(50);
      expect(viewport.scrollTop).toBeGreaterThan(0);
      expect(viewport.scrollTop).toBeLessThan(120);
      expect(frames.size).toBe(1);

      act(() => useSettingsStore.setState({ settings: { "appearance.reduce_motion": "true" } }));
      expect(frames.size).toBe(0);
      expect(wheel.tryAnimatedWheelScroll(viewport, 120)).toBe(false);

      act(() => useSettingsStore.setState({ settings: { "appearance.reduce_motion": "false" } }));
      expect(install).toHaveBeenCalledTimes(2);
      expect(wheel.tryAnimatedWheelScroll(viewport, 120)).toBe(true);
      expect(frames.size).toBe(1);
    } finally {
      view.unmount();
      viewport.remove();
    }
    expect(frames.size).toBe(0);
  });
  it("uses fine input pacing only after the native bridge is confirmed", async () => {
    let resolve!: (available: boolean) => void;
    vi.mocked(invoke).mockReturnValue(new Promise<boolean>(done => { resolve = done; }));
    renderHook(() => useSmoothScrollingInit());
    const available = install.mock.calls[0][1]!;
    expect(available()).toBe(false);
    resolve(true);
    await waitFor(() => expect(available()).toBe(true));
    expect(install).toHaveBeenCalledTimes(1);
  });
  it("retains the baseline when the native capability command is unavailable", async () => {
    vi.mocked(invoke).mockRejectedValue(new Error("Unknown command"));
    renderHook(() => useSmoothScrollingInit());
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_precise_wheel_available"));
    expect(install.mock.calls[0][1]!()).toBe(false);
  });
  it("ignores a native capability reply after teardown", async () => {
    let resolve!: (available: boolean) => void;
    vi.mocked(invoke).mockReturnValue(new Promise<boolean>(done => { resolve = done; }));
    const view = renderHook(() => useSmoothScrollingInit());
    const available = install.mock.calls[0][1]!;
    view.unmount();
    resolve(true);
    await Promise.resolve();
    expect(available()).toBe(false);
    expect(stop).toHaveBeenCalledTimes(1);
  });
  it.each(["true", "false"])("ignores the obsolete %s workaround preference", (value) => {
    useSettingsStore.setState({ settings: { "appearance.smooth_scrolling": value }, loaded: true });
    const view = renderHook(() => useSmoothScrollingInit());
    expect(install).toHaveBeenCalledTimes(1);
    useSettingsStore.setState({ settings: {}, loaded: true });
    view.rerender();
    expect(install).toHaveBeenCalledTimes(1);
    expect(stop).not.toHaveBeenCalled();
  });
  it("disposes pending animation and listeners on window teardown", () => {
    const view = renderHook(() => useSmoothScrollingInit());
    view.unmount();
    expect(stop).toHaveBeenCalledTimes(1);
  });
  it("keeps a Chromium webview's own scrolling", () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("AppleWebKit/537.36 Chrome/146.0.0.0");
    renderHook(() => useSmoothScrollingInit());
    expect(install).not.toHaveBeenCalled();
  });
  it("never installs on a remote browser, even with a Linux WebKit user agent", () => {
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
    try {
      renderHook(() => useSmoothScrollingInit());
      expect(install).not.toHaveBeenCalled();
    } finally { delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__; }
  });
});
