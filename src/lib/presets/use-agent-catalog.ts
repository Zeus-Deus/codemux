import { useEffect, useState } from "react";
import { listAgentCatalog } from "@/tauri/commands";
import type { AgentCatalogEntry } from "@/tauri/types";

// The agent catalog is static for a session, so fetch it once and share
// the result across every editor instance via a module-level cache.
let cache: AgentCatalogEntry[] | null = null;
let inflight: Promise<AgentCatalogEntry[]> | null = null;

/** Fetch (and cache) the agent catalog, returning the cached value if present. */
export function ensureAgentCatalog(): Promise<AgentCatalogEntry[]> {
  if (cache) return Promise.resolve(cache);
  inflight ??= listAgentCatalog().then((c) => {
    cache = c;
    return c;
  });
  return inflight;
}

/** React hook returning the agent catalog, loading it on first use. */
export function useAgentCatalog(): AgentCatalogEntry[] {
  const [catalog, setCatalog] = useState<AgentCatalogEntry[]>(cache ?? []);

  useEffect(() => {
    let active = true;
    ensureAgentCatalog()
      .then((c) => {
        if (active) setCatalog(c);
      })
      .catch(console.error);
    return () => {
      active = false;
    };
  }, []);

  return catalog;
}
