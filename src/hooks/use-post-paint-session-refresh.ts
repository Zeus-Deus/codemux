import { useEffect } from "react";

import type { AuthSessionStatus } from "@/tauri/types";

/** Backoff schedule for re-verifying a still-pending session after the
 *  first post-paint attempt. A transient verification failure leaves the
 *  session `pending-verification` — a rejected invoke via the auth store's
 *  refreshSession catch, and an unreachable verify endpoint with no cached
 *  identity via the backend's resolved still-pending status — so without
 *  these bounded retries a valid token would sit on the login frame until
 *  something else re-triggered verification. Once the schedule is
 *  exhausted, any status change re-arms the effect from scratch. */
export const PENDING_VERIFICATION_RETRY_DELAYS_MS: readonly number[] = [
  2_000, 5_000, 15_000, 30_000,
];

/**
 * Start remote verification after a locally rendered frame when a valid token
 * has no cached user. The authenticated shell cannot mount in this state, so
 * its normal first-paint callback would otherwise never fire. While the
 * session stays pending (a transient verification failure), the attempt is
 * retried on a bounded backoff schedule.
 */
export function usePostPaintPendingSessionRefresh(
  isLoading: boolean,
  sessionStatus: AuthSessionStatus,
  onAfterPaint: () => void,
): void {
  useEffect(() => {
    if (isLoading || sessionStatus !== "pending-verification") return;

    let cancelled = false;
    let secondFrame: number | null = null;
    let retryTimer: number | null = null;
    let retryIndex = 0;

    const scheduleRetry = (): void => {
      if (retryIndex >= PENDING_VERIFICATION_RETRY_DELAYS_MS.length) return;
      const delay = PENDING_VERIFICATION_RETRY_DELAYS_MS[retryIndex];
      retryIndex += 1;
      retryTimer = window.setTimeout(() => {
        if (cancelled) return;
        onAfterPaint();
        scheduleRetry();
      }, delay);
    };
    const requestFrame = (callback: FrameRequestCallback): number => {
      if (typeof window.requestAnimationFrame === "function") {
        return window.requestAnimationFrame(callback);
      }
      return window.setTimeout(() => callback(performance.now()), 16);
    };
    const cancelFrame = (handle: number): void => {
      if (typeof window.cancelAnimationFrame === "function") {
        window.cancelAnimationFrame(handle);
      } else {
        window.clearTimeout(handle);
      }
    };

    const firstFrame = requestFrame(() => {
      secondFrame = requestFrame(() => {
        if (cancelled) return;
        onAfterPaint();
        // Success or an authoritative rejection moves sessionStatus off
        // `pending-verification`, which unmounts this effect and cancels
        // the pending timer — retries only ever fire while still pending.
        scheduleRetry();
      });
    });

    return () => {
      cancelled = true;
      cancelFrame(firstFrame);
      if (secondFrame !== null) cancelFrame(secondFrame);
      if (retryTimer !== null) window.clearTimeout(retryTimer);
    };
  }, [isLoading, onAfterPaint, sessionStatus]);
}
