import { useCallback, useEffect, useRef, useState } from "react";

import { toast } from "@/lib/toast";
import { useRemoteReconnectEpoch } from "@/remote/remote-connection-store";
import { prWatchStart, prWatchStatus, prWatchStop } from "@/tauri/commands";
import { onPrWatchChanged } from "@/tauri/events";
import type { PrWatchStatus } from "@/tauri/types";

/**
 * A chat's pull-request watch, kept live by `pr-watch-changed`, plus the
 * toggle. `toggle` stops an active watch and starts (or re-arms) any other,
 * for `prNumber` when given — the workspace's current pull request — and
 * otherwise for the one it last watched.
 *
 * `loaded` is false until the chat's status is known: an "Off" shown while
 * the first read is in flight may be an active watch, and starting it again
 * would reset its attempts, so the toggle does nothing until then.
 */
export function usePrWatch(threadId: string | null): {
  watch: PrWatchStatus | null;
  loaded: boolean;
  busy: boolean;
  toggle: (prNumber?: number | null) => void;
} {
  const [watch, setWatch] = useState<PrWatchStatus | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  // A remote client misses events while disconnected; reread on reconnect.
  const reconnectEpoch = useRemoteReconnectEpoch();
  // Bumped whenever the chat (or connection) changes. Every async result
  // checks it, so a late answer for the previous chat is dropped.
  const generation = useRef(0);

  useEffect(() => {
    const current = ++generation.current;
    const live = () => generation.current === current;
    setWatch(null);
    setLoaded(false);
    setBusy(false);
    if (!threadId) return;
    // An event can land before the read resolves; the event is newer.
    let heard = false;
    let unlisten: (() => void) | undefined;
    onPrWatchChanged((payload) => {
      if (!live() || payload.thread_id !== threadId) return;
      heard = true;
      setWatch(payload.watch);
      setLoaded(true);
    }).then((fn) => {
      if (live()) unlisten = fn;
      else fn();
    });
    prWatchStatus(threadId)
      .then((status) => {
        if (!live()) return;
        if (!heard) setWatch(status);
        setLoaded(true);
      })
      .catch(() => {});
    return () => {
      // Invalidate before the next effect runs, so nothing lands in between.
      if (live()) generation.current++;
      unlisten?.();
    };
  }, [threadId, reconnectEpoch]);

  const toggle = useCallback(
    (prNumber?: number | null) => {
      if (!threadId || busy || !loaded) return;
      const current = generation.current;
      const live = () => generation.current === current;
      const stopping = watch?.status === "active";
      setBusy(true);
      const done = stopping
        ? prWatchStop(threadId).then(() => {
            if (live()) setWatch(null);
          })
        : prWatchStart(threadId, prNumber ?? watch?.pr_number ?? null).then((status) => {
            if (live()) setWatch(status);
          });
      done
        .catch((error) =>
          toast.failure(stopping ? "Couldn't stop watching" : "Couldn't watch the pull request", error),
        )
        .finally(() => {
          if (live()) setBusy(false);
        });
    },
    [threadId, busy, loaded, watch],
  );

  return { watch, loaded, busy, toggle };
}
