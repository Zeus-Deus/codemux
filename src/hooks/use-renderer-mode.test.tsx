import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, renderHook, waitFor } from "@testing-library/react";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";

import {
  getRendererMode,
  resetTranscriptFadeCacheForTests,
} from "@/components/chat/transcript-fade";

import { useRendererModeInit } from "./use-renderer-mode";

const invokeMock = invoke as unknown as ReturnType<typeof vi.fn>;

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  resetTranscriptFadeCacheForTests();
  delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
});

describe("useRendererModeInit", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("probes the backend on desktop and caches a compatibility answer", async () => {
    invokeMock.mockResolvedValue("compatibility");

    renderHook(() => useRendererModeInit());

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("get_renderer_mode"),
    );
    await waitFor(() => expect(getRendererMode()).toBe("compatibility"));
  });

  it("never probes from the web remote client — the answer would describe the host", () => {
    // `get_renderer_mode` reports the desktop host's webview; the remote UI
    // renders in the user's own (composited) browser, so a compatibility-
    // rendered host must not disable composited-only effects remotely.
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
    invokeMock.mockResolvedValue("compatibility");

    renderHook(() => useRendererModeInit());

    expect(invokeMock).not.toHaveBeenCalled();
    expect(getRendererMode()).toBe("accelerated");
  });
});
