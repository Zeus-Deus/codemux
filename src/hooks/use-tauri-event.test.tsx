import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useTauriEvent } from "./use-tauri-event";

afterEach(cleanup);

describe("useTauriEvent lifecycle", () => {
  it("subscribes only while enabled and resumes when re-enabled", async () => {
    const stop = vi.fn();
    const subscribe = vi.fn(async () => stop);
    const { rerender } = renderHook(({ enabled }) => useTauriEvent(subscribe, vi.fn(), [], enabled), { initialProps: { enabled: false } });
    expect(subscribe).not.toHaveBeenCalled();
    rerender({ enabled: true });
    await act(async () => {});
    expect(subscribe).toHaveBeenCalledOnce();
    rerender({ enabled: false });
    expect(stop).toHaveBeenCalledOnce();
    rerender({ enabled: true });
    expect(subscribe).toHaveBeenCalledTimes(2);
  });

  it("delivers active events and disposes the listener on unmount", async () => {
    let deliver!: (value: string) => void;
    const stop = vi.fn();
    const subscribe = vi.fn(async (handler: (value: string) => void) => { deliver = handler; return stop; });
    const handler = vi.fn();
    const { unmount } = renderHook(() => useTauriEvent(subscribe, handler));
    await act(async () => {});
    deliver("active");
    expect(handler).toHaveBeenCalledWith("active");
    unmount();
    expect(stop).toHaveBeenCalledOnce();
  });

  it("ignores late events while registration is still finishing after unmount", async () => {
    let deliver!: (value: string) => void;
    let resolve!: (stop: () => void) => void;
    const subscribe = (handler: (value: string) => void) => {
      deliver = handler;
      return new Promise<() => void>((done) => { resolve = done; });
    };
    const handler = vi.fn();
    const stop = vi.fn();
    const { unmount } = renderHook(() => useTauriEvent(subscribe, handler));
    unmount();
    deliver("late");
    await act(async () => { resolve(stop); });
    expect(handler).not.toHaveBeenCalled();
    expect(stop).toHaveBeenCalledOnce();
  });

  it("does not deliver events through an obsolete dependency subscription", async () => {
    const handlers: ((value: string) => void)[] = [];
    const subscribe = async (handler: (value: string) => void) => { handlers.push(handler); return () => {}; };
    const handler = vi.fn();
    const { rerender } = renderHook(({ key }) => useTauriEvent(subscribe, handler, [key]), { initialProps: { key: "old" } });
    rerender({ key: "current" });
    handlers[0]("obsolete");
    handlers[1]("current");
    expect(handler.mock.calls).toEqual([["current"]]);
  });

  it.each(["throws", "rejects"] as const)("reports registration that %s without crashing the hook", async (kind) => {
    const error = new Error("disconnected");
    const report = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      const subscribe = (): Promise<() => void> => {
        if (kind === "throws") throw error;
        return Promise.reject(error);
      };
      renderHook(() => useTauriEvent(subscribe, vi.fn()));
      await act(async () => {});
      expect(report).toHaveBeenCalledWith("[tauri-event] Failed to register listener:", error);
    } finally {
      report.mockRestore();
    }
  });
});
