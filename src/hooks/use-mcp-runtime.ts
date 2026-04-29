import { useEffect, useState } from "react";

import { listen } from "@tauri-apps/api/event";

import {
  getMcpRuntimeStatus,
  MCP_STATUS_CHANGED_EVENT,
  type McpServerRuntime,
} from "@/tauri/commands";

/**
 * Live MCP runtime snapshot. Hydrates once via `getMcpRuntimeStatus`
 * on mount, then listens to `mcp-status-changed` events so the UI
 * doesn't poll. Each event payload is one server's row; we merge it
 * into the existing list keyed by `id`.
 *
 * Returns:
 *   - `runtimes`: keyed by server id for O(1) row lookups.
 *   - `loaded`: `true` once the initial fetch returns. Lets the UI
 *     show a transitional state without flickering.
 */
export function useMcpRuntime() {
  const [runtimes, setRuntimes] = useState<Map<string, McpServerRuntime>>(
    () => new Map(),
  );
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    void getMcpRuntimeStatus()
      .then((rows) => {
        if (cancelled) return;
        setRuntimes(new Map(rows.map((r) => [r.id, r])));
        setLoaded(true);
      })
      .catch((err) => {
        if (cancelled) return;
        console.warn("[mcp] getMcpRuntimeStatus failed:", err);
        setLoaded(true);
      });

    // `listen()` throws synchronously when `window.__TAURI_INTERNALS__`
    // is missing (jsdom test envs that don't mock the Tauri shim).
    // Wrap in try/catch so the hook degrades to a static snapshot
    // instead of crashing every component that mounts a Composer.
    try {
      void listen<McpServerRuntime>(MCP_STATUS_CHANGED_EVENT, (event) => {
        if (cancelled) return;
        const row = event.payload;
        setRuntimes((prev) => {
          const next = new Map(prev);
          next.set(row.id, row);
          return next;
        });
      })
        .then((dispose) => {
          if (cancelled) {
            dispose();
            return;
          }
          unlisten = dispose;
        })
        .catch((err) => {
          console.warn("[mcp] listen failed:", err);
        });
    } catch (err) {
      console.warn("[mcp] listen threw synchronously:", err);
    }

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  return { runtimes, loaded };
}
