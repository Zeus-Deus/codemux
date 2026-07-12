/**
 * Global terminal scrollback serialization manager.
 *
 * Mounted TerminalPane instances register their serialize callbacks here.
 * When a pane unmounts (tab/workspace switch), it caches its serialized buffer
 * in the Rust backend so it survives React component lifecycle.
 *
 * On the "serialize-terminal-buffers" event (emitted by the backend on close):
 * 1. All currently mounted panes serialize live data → saved to disk
 * 2. The backend flushes cached data for unmounted panes → also saved to disk
 * 3. Frontend emits "scrollback-serialization-complete" to unblock close
 */

import { useEffect } from "react";
import type { ScrollbackPayload } from "@/tauri/commands";
import {
  saveTerminalScrollback,
  flushScrollbackCache,
} from "@/tauri/commands";
import {
  onSerializeTerminalBuffers,
  emitScrollbackSerializationComplete,
} from "@/tauri/events";
import { isRemoteClient } from "@/components/remote/is-remote-client";

type SerializeCallback = () => ScrollbackPayload | null;

// Global registry of terminal serialize callbacks, keyed by sessionId.
const serializeRegistry = new Map<string, SerializeCallback>();

/**
 * Register a terminal pane's serialize callback.
 * Called by TerminalPane on mount. Returns an unregister function.
 */
export function registerTerminalForSerialize(
  sessionId: string,
  callback: SerializeCallback,
): () => void {
  serializeRegistry.set(sessionId, callback);
  return () => {
    serializeRegistry.delete(sessionId);
  };
}

/**
 * Hook that listens for the serialize event and coordinates all registered terminals.
 * Mount this ONCE at the app level.
 */
export function useScrollbackSerializer() {
  useEffect(() => {
    // The desktop window owns terminal-scrollback serialization. A remote
    // client must never listen for `serialize-terminal-buffers`, never write
    // scrollback files, and never ack `scrollback-serialization-complete` —
    // otherwise a browser client would race the desktop for ownership and its
    // ack could unblock the desktop's close before the desktop finished saving.
    if (isRemoteClient()) return;

    let unlisten: (() => void) | null = null;

    onSerializeTerminalBuffers(async () => {
      // Step 1: serialize all currently mounted (live) panes
      const promises: Promise<void | unknown>[] = [];

      for (const [, callback] of serializeRegistry) {
        const payload = callback();
        if (payload && payload.data) {
          promises.push(
            saveTerminalScrollback(payload).catch((err) => {
              console.error("[scrollback] Failed to save live pane:", err);
            }),
          );
        }
      }

      await Promise.allSettled(promises);

      // Step 2: flush cached data for unmounted panes (inactive tabs/workspaces)
      try {
        await flushScrollbackCache();
      } catch (err) {
        console.error("[scrollback] Failed to flush cache:", err);
      }

      // Step 3: signal the backend that serialization is done
      await emitScrollbackSerializationComplete();
    }).then((fn) => {
      unlisten = fn;
    });

    return () => {
      unlisten?.();
    };
  }, []);
}
