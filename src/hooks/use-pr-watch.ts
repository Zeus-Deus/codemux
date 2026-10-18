import { useCallback, useEffect, useRef, useState } from "react";

import { toast } from "@/lib/toast";
import { useRemoteReconnectEpoch } from "@/remote/remote-connection-store";
import { prWatchStart, prWatchStatus, prWatchStop } from "@/tauri/commands";
import { onPrWatchChanged } from "@/tauri/events";
import type { PrWatchStatus } from "@/tauri/types";

/** Where the chat's status stands: an "Off" shown before it is known may be
 *  an active watch, and starting it again would reset its attempts. */
export type PrWatchLoad = "loading" | "loaded" | "failed";

/**
 * A chat's pull-request watch, kept live by `pr-watch-changed`, plus the
 * toggle. `toggle` stops an active watch and starts (or re-arms) any other,
 * for `prNumber` when given — the workspace's current pull request — and
 * otherwise for the one it last watched. It does nothing until the status
 * is known; after a failed read it reads again instead.
 *
 * The backend emits `pr-watch-changed` for every change, including the
 * caller's own start or stop, so events are the authority: a command's
 * reply is applied only when no event for this chat arrived since the
 * click, and nothing from a previous chat or connection is applied at all.
 */
export function usePrWatch(threadId: string | null): {
  watch: PrWatchStatus | null;
  load: PrWatchLoad;
  busy: boolean;
  toggle: (prNumber?: number | null) => void;
} {
  const [watch, setWatch] = useState<PrWatchStatus | null>(null);
  const [load, setLoad] = useState<PrWatchLoad>("loading");
  const [busy, setBusy] = useState(false);
  const [attempt, setAttempt] = useState(0);
  // A remote client misses events while disconnected; reread on reconnect.
  const reconnectEpoch = useRemoteReconnectEpoch();
  // Bumped whenever the chat, connection or read attempt changes.
  const generation = useRef(0);
  // Counts this chat's events, so a reply can tell whether it is stale.
  const events = useRef(0);

  useEffect(() => {
    const current = ++generation.current;
    const live = () => generation.current === current;
    setWatch(null);
    setLoad("loading");
    setBusy(false);
    if (!threadId) return;
    let unlisten: (() => void) | undefined;
    // Listen first, then read: an event that lands between the read and
    // the registration would otherwise be lost.
    onPrWatchChanged((payload) => {
      if (!live() || payload.thread_id !== threadId) return;
      events.current++;
      setWatch(payload.watch);
      setLoad("loaded");
    })
      .then((fn) => {
        if (!live()) {
          fn();
          return;
        }
        unlisten = fn;
        const seen = events.current;
        return prWatchStatus(threadId).then((status) => {
          if (!live()) return;
          if (events.current === seen) setWatch(status);
          setLoad("loaded");
        });
      })
      .catch(() => {
        if (live()) setLoad((load) => (load === "loaded" ? load : "failed"));
      });
    return () => {
      // Invalidate before the next effect runs, so nothing lands in between.
      if (live()) generation.current++;
      unlisten?.();
    };
  }, [threadId, reconnectEpoch, attempt]);

  const toggle = useCallback(
    (prNumber?: number | null) => {
      if (!threadId || busy) return;
      if (load === "failed") {
        setAttempt((n) => n + 1);
        return;
      }
      if (load !== "loaded") return;
      const current = generation.current;
      const seen = events.current;
      const apply = (next: PrWatchStatus | null) => {
        if (generation.current === current && events.current === seen) setWatch(next);
      };
      const stopping = watch?.status === "active";
      setBusy(true);
      const done = stopping
        ? prWatchStop(threadId).then(() => apply(null))
        : prWatchStart(threadId, prNumber ?? watch?.pr_number ?? null).then(apply);
      done
        .catch((error) =>
          toast.failure(stopping ? "Couldn't stop watching" : "Couldn't watch the pull request", error),
        )
        .finally(() => {
          if (generation.current === current) setBusy(false);
        });
    },
    [threadId, busy, load, watch],
  );

  return { watch, load, busy, toggle };
}
