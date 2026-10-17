import { StrictMode } from "react";
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ScrollbackPayload } from "@/tauri/commands";

const mocks = vi.hoisted(() => ({
  subscribe: vi.fn<(callback: () => Promise<void>) => Promise<() => void>>(),
  save: vi.fn<(...args: unknown[]) => Promise<void>>(),
  flush: vi.fn<() => Promise<void>>(),
  complete: vi.fn<() => Promise<void>>(),
  remote: false,
}));

vi.mock("@/tauri/commands", () => ({ saveTerminalScrollback: mocks.save, flushScrollbackCache: mocks.flush }));
vi.mock("@/tauri/events", () => ({ onSerializeTerminalBuffers: mocks.subscribe, emitScrollbackSerializationComplete: mocks.complete }));
vi.mock("@/components/remote/is-remote-client", () => ({ isRemoteClient: () => mocks.remote }));

import { registerTerminalForSerialize, useScrollbackSerializer } from "./use-scrollback-serializer";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function payload(sessionId: string): ScrollbackPayload {
  return {
    pane_id: `pane-${sessionId}`, session_id: sessionId, workspace_id: "ws",
    working_directory: "/repo", original_command: null, cols: 80, rows: 24,
    data: "latest terminal text", adapter_captures: {}, adapter_id: null, alternate_buffer: false,
  };
}

describe("terminal scrollback close serialization", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    mocks.remote = false;
    mocks.subscribe.mockResolvedValue(() => {});
    mocks.save.mockResolvedValue();
    mocks.flush.mockResolvedValue();
    mocks.complete.mockResolvedValue();
  });

  afterEach(cleanup);

  it("disposes a listener whose registration finishes after unmount", async () => {
    const pending = deferred<() => void>();
    const stop = vi.fn();
    mocks.subscribe.mockReturnValueOnce(pending.promise);
    const { unmount } = renderHook(() => useScrollbackSerializer());
    unmount();
    await act(async () => { pending.resolve(stop); });
    expect(stop).toHaveBeenCalledOnce();
  });

  it("leaves one active listener after Strict Mode's effect replay", async () => {
    const stops = [vi.fn(), vi.fn()];
    mocks.subscribe.mockImplementationOnce(async () => stops[0]).mockImplementationOnce(async () => stops[1]);
    const { unmount } = renderHook(() => useScrollbackSerializer(), { wrapper: StrictMode });
    await act(async () => {});
    expect(mocks.subscribe).toHaveBeenCalledTimes(2);
    expect(stops[0]).toHaveBeenCalledOnce();
    expect(stops[1]).not.toHaveBeenCalled();
    unmount();
    expect(stops[1]).toHaveBeenCalledOnce();
  });

  it("does not unregister a replacement terminal's serializer", async () => {
    renderHook(() => useScrollbackSerializer());
    const stale = registerTerminalForSerialize("same-session", () => null);
    const saved = payload("same-session");
    const current = registerTerminalForSerialize("same-session", () => saved);
    try {
      stale();
      await mocks.subscribe.mock.calls[0][0]();
      expect(mocks.save).toHaveBeenCalledWith(saved);
    } finally {
      current();
    }
  });

  it("waits for every live save and cache flush before acknowledging close", async () => {
    renderHook(() => useScrollbackSerializer());
    const pendingSave = deferred<void>();
    const pendingFlush = deferred<void>();
    mocks.save.mockReturnValueOnce(pendingSave.promise);
    mocks.flush.mockReturnValueOnce(pendingFlush.promise);
    const unregister = registerTerminalForSerialize("live", () => payload("live"));
    try {
      const close = mocks.subscribe.mock.calls[0][0]();
      expect(mocks.flush).not.toHaveBeenCalled();
      pendingSave.resolve();
      await act(async () => {});
      expect(mocks.flush).toHaveBeenCalledOnce();
      expect(mocks.complete).not.toHaveBeenCalled();
      pendingFlush.resolve();
      await close;
      expect(mocks.complete).toHaveBeenCalledOnce();
    } finally {
      unregister();
    }
  });

  it("does not register or acknowledge close from a remote client", () => {
    mocks.remote = true;
    renderHook(() => useScrollbackSerializer());
    expect(mocks.subscribe).not.toHaveBeenCalled();
    expect(mocks.complete).not.toHaveBeenCalled();
  });
});
