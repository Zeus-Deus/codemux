export type IdlePrefetchLoader = () => Promise<unknown>;

const IDLE_TIMEOUT_MS = 2_000;
const FALLBACK_DELAY_MS = 1_000;

/**
 * Start a small list of dynamic imports only once the renderer is idle.
 * Loaders run sequentially to avoid a post-paint burst of download, parse,
 * and compile work. Cancellation prevents every loader that has not started;
 * an import already in flight cannot be cancelled by the platform.
 */
export function scheduleSequentialIdlePrefetch(
  loaders: readonly IdlePrefetchLoader[],
): () => void {
  let cancelled = false;
  let idleHandle: number | null = null;
  let timeoutHandle: ReturnType<typeof setTimeout> | null = null;

  const run = async () => {
    for (const load of loaders) {
      if (cancelled) return;
      try {
        await load();
      } catch (error) {
        // Prefetch is opportunistic. A real navigation retries through the
        // React.lazy boundary, whose error fallback gives the user recovery.
        console.warn("[prefetch] lazy chunk failed:", error);
      }
    }
  };

  if (typeof requestIdleCallback === "function") {
    idleHandle = requestIdleCallback(() => {
      idleHandle = null;
      if (!cancelled) void run();
    }, { timeout: IDLE_TIMEOUT_MS });
  } else {
    timeoutHandle = setTimeout(() => {
      timeoutHandle = null;
      if (!cancelled) void run();
    }, FALLBACK_DELAY_MS);
  }

  return () => {
    cancelled = true;
    if (idleHandle !== null && typeof cancelIdleCallback === "function") {
      cancelIdleCallback(idleHandle);
      idleHandle = null;
    }
    if (timeoutHandle !== null) {
      clearTimeout(timeoutHandle);
      timeoutHandle = null;
    }
  };
}
