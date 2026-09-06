import { Activity, createContext, useContext, useLayoutEffect, useMemo, useReducer, useRef, type ReactNode } from "react";
import { createPortal } from "react-dom";

type ScrollPosition = { node: HTMLElement; top: number; left: number };
type Entry = {
  key: string;
  host: HTMLDivElement;
  children: ReactNode;
  slot: HTMLDivElement | null;
  scroll: ScrollPosition[];
};

function RestoreScroll({ entry }: { entry: Entry }) {
  // This sibling precedes the transcript inside Activity. Its reconnect effect
  // runs after React unhides DOM, but before the virtualizer reads geometry.
  // Restore once only: subsequent list-owned sends/jumps/following-end win.
  useLayoutEffect(() => {
    for (const { node, top, left } of entry.scroll) {
      if (entry.host.contains(node)) { node.scrollTop = top; node.scrollLeft = left; }
    }
    entry.scroll = [];
  }, [entry]);
  return null;
}
type Cache = {
  publish: (key: string, slot: HTMLDivElement, children: ReactNode) => void;
  release: (key: string, slot: HTMLDivElement) => void;
};
const CacheContext = createContext<Cache | null>(null);

/** React owns portal children. The cache moves ONLY its host, never children
 * or React-owned slot/parking nodes. Both attachment targets are React-empty. */
export function TranscriptCacheProvider({ children, activeKey, validKeys }: {
  children: ReactNode;
  activeKey: string | null;
  validKeys: readonly string[];
}) {
  const entries = useRef(new Map<string, Entry>());
  const parking = useRef<HTMLDivElement>(null);
  const [, refresh] = useReducer((n: number) => n + 1, 0);
  const cache = useMemo<Cache>(() => ({
    publish(key, slot, children) {
      let entry = entries.current.get(key);
      if (!entry) {
        const host = document.createElement("div");
        host.className = "h-full min-h-0 w-full";
        host.dataset.transcriptCacheHost = key;
        entry = { key, host, children, slot, scroll: [] };
        entries.current.set(key, entry);
      }
      // Map insertion order is recency; only commits update it.
      entries.current.delete(key);
      entries.current.set(key, entry);
      entry.children = children;
      entry.slot = slot;
      // Publish in a layout effect, before rendering/revealing the portal. Its
      // descendant layout effects therefore measure the REAL active slot.
      if (entry.host.parentNode !== slot) slot.appendChild(entry.host);
      refresh();
    },
    release(key, slot) {
      const entry = entries.current.get(key);
      if (!entry || entry.slot !== slot) return;
      // Snapshot before parking can clamp browser scroll offsets to zero.
      // Include nested horizontal scrollers, not only the virtual viewport.
      entry.scroll = [...entry.host.querySelectorAll<HTMLElement>("*")]
        .filter((node) => node.scrollTop !== 0 || node.scrollLeft !== 0)
        .map((node) => ({ node, top: node.scrollTop, left: node.scrollLeft }));
      entry.slot = null;
      parking.current?.appendChild(entry.host);
      refresh();
    },
  }), []);
  const snapshot = [...entries.current.values()];
  const valid = new Set(validKeys);
  const leased = snapshot.filter((entry) => valid.has(entry.key) && entry.slot !== null);
  const inactive = snapshot.filter((entry) => valid.has(entry.key) && entry.slot === null);
  const budget = Math.max(0, 4 - leased.length);
  const retained = new Set([...leased, ...(budget ? inactive.slice(-budget) : [])]);
  // Children of victims were removed by React's mutation phase. Detach ONLY
  // their now-empty imperative hosts; no removeChild/innerHTML on React DOM.
  useLayoutEffect(() => {
    // React restores selection after DOM mutations. Blur AFTER that restoration
    // if it followed the moved node into parking; never restore focus on reveal.
    const focused = parking.current?.ownerDocument.activeElement;
    if (focused instanceof HTMLElement && parking.current?.contains(focused)) focused.blur();
    for (const entry of snapshot) {
      if (!retained.has(entry) && (!entry.slot || !valid.has(entry.key))) {
        entries.current.delete(entry.key);
        entry.host.remove();
      }
    }
  });
  return <CacheContext.Provider value={cache}>
    {children}
    <div ref={parking} data-transcript-cache-parking="" hidden inert aria-hidden="true" />
    {[...retained].map((entry) => createPortal(
      <Activity mode={entry.slot && entry.key === activeKey ? "visible" : "hidden"}>
        <RestoreScroll entry={entry} />
        {entry.children}
      </Activity>, entry.host, entry.key,
    ))}
  </CacheContext.Provider>;
}

export function TranscriptCacheMount({ children, cacheKey }: { children: ReactNode; cacheKey: string }) {
  const cache = useContext(CacheContext);
  const slot = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const node = slot.current;
    if (!cache || !node) return;
    return () => cache.release(cacheKey, node);
  }, [cache, cacheKey]);
  useLayoutEffect(() => {
    if (cache && slot.current) cache.publish(cacheKey, slot.current, children);
  }, [cache, cacheKey, children]);
  return <div ref={slot} data-transcript-cache-slot="" className="h-full min-h-0 w-full">{cache ? null : children}</div>;
}
