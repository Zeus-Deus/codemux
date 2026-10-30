import { useEffect, useState } from "react";

import { useRemoteReconnectEpoch } from "@/remote/remote-connection-store";
import { agentSecretRequests } from "@/tauri/commands";
import { onSecretRequestsChanged } from "@/tauri/events";
import type { SecretRequest } from "@/tauri/types";

/**
 * A chat's open secret cards, kept live by `secret-requests-changed`. The
 * backend emits the full open list on every change, so the latest event (or
 * read) is the whole truth.
 */
export function useSecretRequests(threadId: string | null): SecretRequest[] {
  const [requests, setRequests] = useState<SecretRequest[]>([]);
  // A remote client misses events while disconnected; reread on reconnect.
  const reconnectEpoch = useRemoteReconnectEpoch();

  useEffect(() => {
    setRequests([]);
    if (!threadId) return;
    let live = true;
    let evented = false;
    let unlisten: (() => void) | undefined;
    // Listen first, then read: an event that lands between the read and
    // the registration would otherwise be lost.
    void onSecretRequestsChanged((payload) => {
      if (!live || payload.thread_id !== threadId) return;
      evented = true;
      setRequests(payload.requests);
    })
      .then((fn) => {
        if (!live) {
          fn();
          return;
        }
        unlisten = fn;
        return agentSecretRequests(threadId).then((list) => {
          // An event since the read started is newer than the read.
          if (live && !evented) setRequests(list);
        });
      })
      .catch(() => {
        /* No backend (tests, storybook): there are no cards to show. */
      });
    return () => {
      live = false;
      unlisten?.();
    };
  }, [threadId, reconnectEpoch]);

  return requests;
}
