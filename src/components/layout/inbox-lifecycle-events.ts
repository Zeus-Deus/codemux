/** A lifecycle transition the inbox may want to animate. The writes have
 *  already landed in the store by the time listeners hear about them; this
 *  channel only carries the *motion* (rise-in rows, the "woke" badge), which
 *  belongs to whichever view is on screen. */
export type InboxLifecycleEvent =
  | { kind: "resurfaced"; workspaceId: string }
  | {
      kind: "woke";
      workspaceId: string;
      reason: "activity" | "timer";
      wasPinned: boolean;
    }
  | { kind: "auto-settled"; workspaceId: string };

const listeners = new Set<(event: InboxLifecycleEvent) => void>();

/** Hear lifecycle transitions made by `useInboxLifecycle` (`./use-inbox-lifecycle`). Returns the
 *  unsubscribe. */
export function subscribeInboxLifecycle(
  listener: (event: InboxLifecycleEvent) => void,
): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function emitInboxLifecycle(event: InboxLifecycleEvent): void {
  for (const listener of listeners) listener(event);
}
