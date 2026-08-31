import { renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  PENDING_VERIFICATION_RETRY_DELAYS_MS,
  usePostPaintPendingSessionRefresh,
} from "./use-post-paint-session-refresh";
import type { AuthSessionStatus } from "@/tauri/types";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

function stubFrameQueue(): FrameRequestCallback[] {
  const frames: FrameRequestCallback[] = [];
  vi.stubGlobal(
    "requestAnimationFrame",
    vi.fn((callback: FrameRequestCallback) => {
      frames.push(callback);
      return frames.length;
    }),
  );
  vi.stubGlobal("cancelAnimationFrame", vi.fn());
  return frames;
}

describe("usePostPaintPendingSessionRefresh", () => {
  it("verifies a valid token with no cached user after the login frame paints", () => {
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal(
      "requestAnimationFrame",
      vi.fn((callback: FrameRequestCallback) => {
        frames.push(callback);
        return frames.length;
      }),
    );
    vi.stubGlobal("cancelAnimationFrame", vi.fn());
    const refresh = vi.fn();

    renderHook(() =>
      usePostPaintPendingSessionRefresh(
        false,
        "pending-verification",
        refresh,
      ),
    );

    expect(refresh).not.toHaveBeenCalled();
    expect(frames).toHaveLength(1);
    frames.shift()?.(16);
    expect(frames).toHaveLength(1);
    expect(refresh).not.toHaveBeenCalled();
    frames.shift()?.(32);
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it.each<AuthSessionStatus>([
    "local",
    "verified",
    "offline",
    "degraded",
    "signed-out",
  ])("does not verify a %s session from the login-frame path", (status) => {
    const requestFrame = vi.fn();
    vi.stubGlobal("requestAnimationFrame", requestFrame);

    renderHook(() =>
      usePostPaintPendingSessionRefresh(false, status, vi.fn()),
    );

    expect(requestFrame).not.toHaveBeenCalled();
  });

  it("retries with bounded backoff while the session stays pending", () => {
    vi.useFakeTimers();
    const frames = stubFrameQueue();
    const refresh = vi.fn();

    renderHook(() =>
      usePostPaintPendingSessionRefresh(
        false,
        "pending-verification",
        refresh,
      ),
    );
    frames.shift()?.(16);
    frames.shift()?.(32);
    expect(refresh).toHaveBeenCalledTimes(1);

    // A transient verification failure leaves the session pending, so the
    // effect stays mounted and each backoff step re-attempts verification.
    PENDING_VERIFICATION_RETRY_DELAYS_MS.forEach((delay, index) => {
      vi.advanceTimersByTime(delay);
      expect(refresh).toHaveBeenCalledTimes(index + 2);
    });

    // The schedule is bounded: once exhausted, no further retries fire.
    vi.advanceTimersByTime(60 * 60 * 1000);
    expect(refresh).toHaveBeenCalledTimes(
      PENDING_VERIFICATION_RETRY_DELAYS_MS.length + 1,
    );
  });

  it("keeps retrying when a resolved refresh leaves the session pending", () => {
    vi.useFakeTimers();
    const frames = stubFrameQueue();
    const refresh = vi.fn();

    const { rerender } = renderHook(
      ({ status }: { status: AuthSessionStatus }) =>
        usePostPaintPendingSessionRefresh(false, status, refresh),
      {
        initialProps: {
          status: "pending-verification" as AuthSessionStatus,
        },
      },
    );
    frames.shift()?.(16);
    frames.shift()?.(32);
    expect(refresh).toHaveBeenCalledTimes(1);

    // An unreachable verify endpoint with no cached identity RESOLVES with a
    // still-pending status (it does not reject). The status value is
    // unchanged, so the effect must keep its scheduled retries running
    // through the whole schedule.
    PENDING_VERIFICATION_RETRY_DELAYS_MS.forEach((delay, index) => {
      rerender({ status: "pending-verification" });
      vi.advanceTimersByTime(delay);
      expect(refresh).toHaveBeenCalledTimes(index + 2);
    });
  });

  it("stops retrying when a resolved refresh lands offline with a cached user", () => {
    vi.useFakeTimers();
    const frames = stubFrameQueue();
    const refresh = vi.fn();

    const { rerender } = renderHook(
      ({ status }: { status: AuthSessionStatus }) =>
        usePostPaintPendingSessionRefresh(false, status, refresh),
      {
        initialProps: {
          status: "pending-verification" as AuthSessionStatus,
        },
      },
    );
    frames.shift()?.(16);
    frames.shift()?.(32);
    expect(refresh).toHaveBeenCalledTimes(1);

    rerender({ status: "offline" });
    vi.advanceTimersByTime(60 * 60 * 1000);
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("cancels scheduled retries once the session leaves pending", () => {
    vi.useFakeTimers();
    const frames = stubFrameQueue();
    const refresh = vi.fn();

    const { rerender } = renderHook(
      ({ status }: { status: AuthSessionStatus }) =>
        usePostPaintPendingSessionRefresh(false, status, refresh),
      {
        initialProps: {
          status: "pending-verification" as AuthSessionStatus,
        },
      },
    );
    frames.shift()?.(16);
    frames.shift()?.(32);
    expect(refresh).toHaveBeenCalledTimes(1);

    rerender({ status: "verified" });
    vi.advanceTimersByTime(60 * 60 * 1000);
    expect(refresh).toHaveBeenCalledTimes(1);
  });
});
