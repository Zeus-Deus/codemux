import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, renderHook, waitFor } from "@testing-library/react";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@/tauri/commands", () => ({
  dbGetAllSettings: vi.fn(async () => ({})),
  dbSetSetting: vi.fn(async () => undefined),
}));

import { invoke } from "@tauri-apps/api/core";
import { useSettingsStore } from "@/stores/settings-store";

import { applySmoothScrolling, useSmoothScrollingInit } from "./use-smooth-scrolling";

const invokeMock = invoke as unknown as ReturnType<typeof vi.fn>;

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("useSmoothScrollingInit", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(undefined);
    useSettingsStore.setState({ settings: {}, loaded: false });
  });

  it("does not call the webview before the settings have loaded", () => {
    useSettingsStore.setState({
      settings: { "appearance.smooth_scrolling": "true" },
      loaded: false,
    });

    renderHook(() => useSmoothScrollingInit());

    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("re-applies a persisted ON once the settings load", async () => {
    useSettingsStore.setState({
      settings: { "appearance.smooth_scrolling": "true" },
      loaded: true,
    });

    renderHook(() => useSmoothScrollingInit());

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("set_smooth_scrolling", {
        enabled: true,
      }),
    );
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });

  it("stays silent for the default OFF — that is already the native default", () => {
    useSettingsStore.setState({ settings: {}, loaded: true });

    renderHook(() => useSmoothScrollingInit());

    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("never touches the webview from the web remote client", () => {
    // On the remote client `set_smooth_scrolling` would reconfigure the
    // desktop host's webview, not the browser the user is scrolling in.
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
    try {
      useSettingsStore.setState({
        settings: { "appearance.smooth_scrolling": "true" },
        loaded: true,
      });

      renderHook(() => useSmoothScrollingInit());

      expect(invokeMock).not.toHaveBeenCalled();
    } finally {
      delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
    }
  });
});

describe("applySmoothScrolling", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("swallows a missing/failing command instead of throwing", async () => {
    invokeMock.mockRejectedValue(new Error("no such command"));
    vi.spyOn(console, "error").mockImplementation(() => {});

    await expect(applySmoothScrolling(true)).resolves.toBeUndefined();
    expect(console.error).toHaveBeenCalled();
  });
});
