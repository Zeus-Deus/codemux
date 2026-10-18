import { useCallback, useEffect, useRef, useState } from "react";

import { toast } from "@/lib/toast";
import { prWatchStart, prWatchStatus, prWatchStop } from "@/tauri/commands";
import { onPrWatchChanged } from "@/tauri/events";
import type { PrWatchStatus } from "@/tauri/types";

/**
 * A chat's pull-request watch, kept live by `pr-watch-changed`, plus the
 * toggle. `toggle` stops an active watch and starts (or re-arms) any other,
 * for `prNumber` when given — the workspace's current pull request — and
 * otherwise for the one it last watched.
 */
export function usePrWatch(threadId: string | null): {
  watch: PrWatchStatus | null;
  busy: boolean;
  toggle: (prNumber?: number | null) => void;
} {
  const [watch, setWatch] = useState<PrWatchStatus | null>(null);
  const [busy, setBusy] = useState(false);
  // An event can land before the initial read resolves; the event is newer.
  const fresh = useRef(false);

  useEffect(() => {
    setWatch(null);
    if (!threadId) return;
    fresh.current = false;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    onPrWatchChanged((payload) => {
      if (payload.thread_id !== threadId) return;
      fresh.current = true;
      setWatch(payload.watch);
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    prWatchStatus(threadId)
      .then((status) => {
        if (!cancelled && !fresh.current) setWatch(status);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [threadId]);

  const toggle = useCallback((prNumber?: number | null) => {
    if (!threadId || busy) return;
    const stopping = watch?.status === "active";
    setBusy(true);
    const done = stopping
      ? prWatchStop(threadId).then(() => {
          fresh.current = true;
          setWatch(null);
        })
      : prWatchStart(threadId, prNumber ?? watch?.pr_number ?? null).then((status) => {
          fresh.current = true;
          setWatch(status);
        });
    done
      .catch((error) =>
        toast.failure(stopping ? "Couldn't stop watching" : "Couldn't watch the pull request", error),
      )
      .finally(() => setBusy(false));
  }, [threadId, busy, watch]);

  return { watch, busy, toggle };
}
