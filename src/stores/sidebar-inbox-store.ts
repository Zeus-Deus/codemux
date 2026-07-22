import { create } from "zustand";
import { dbGetUiState, dbSetUiState } from "@/tauri/commands";

/** Persisted UI-state key holding the JSON settled list. "Settled" is a
 *  purely visual inbox flag — the workspace is untouched (no archive, no
 *  close); its card just collapses to a one-line row under the "Settled"
 *  divider until the user un-settles it. */
export const SETTLED_UI_STATE_KEY = "sidebar.inbox.settled";

export interface SettledEntry {
  id: string;
  /** When the user settled it (ms epoch) — drives the row's elapsed label. */
  at: number;
}

interface SidebarInboxStore {
  /** True once the persisted settled list has been read (or failed to read —
   *  either way the inbox can render without flashing settled rows late). */
  loaded: boolean;
  /** Workspaces the user has settled, newest-settled first (matches the
   *  visual order of the settled section). Persisted. */
  settled: SettledEntry[];
  /** Active repo filter — a project path, or null for "All". Session-only:
   *  a fresh launch always starts on All. */
  filter: string | null;
  /** Load the persisted settled list once at inbox mount. Idempotent. */
  load: () => Promise<void>;
  settle: (workspaceId: string) => void;
  unsettle: (workspaceId: string) => void;
  setFilter: (projectPath: string | null) => void;
  /** Drop settled entries whose workspace no longer exists (archived /
   *  deleted / closed) so the persisted list can't grow without bound. Only
   *  prunes — and only persists — when something actually vanished. */
  prune: (validIds: Set<string>) => void;
}

let loadPromise: Promise<void> | null = null;

function persist(entries: SettledEntry[]): void {
  dbSetUiState(SETTLED_UI_STATE_KEY, JSON.stringify(entries)).catch(
    console.error,
  );
}

function parseSettled(raw: string | null): SettledEntry[] {
  if (!raw) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (x): x is SettledEntry =>
        typeof x === "object" &&
        x !== null &&
        typeof (x as SettledEntry).id === "string" &&
        typeof (x as SettledEntry).at === "number",
    );
  } catch {
    // Corrupt value — start clean rather than crash the sidebar.
    return [];
  }
}

export const useSidebarInboxStore = create<SidebarInboxStore>((set, get) => ({
  loaded: false,
  settled: [],
  filter: null,

  load: () => {
    if (loadPromise) return loadPromise;
    loadPromise = dbGetUiState(SETTLED_UI_STATE_KEY)
      .then((raw) => {
        set({ settled: parseSettled(raw), loaded: true });
      })
      .catch(() => {
        set({ loaded: true });
      });
    return loadPromise;
  },

  settle: (workspaceId) => {
    const prev = get().settled;
    if (prev.some((e) => e.id === workspaceId)) return;
    const next = [{ id: workspaceId, at: Date.now() }, ...prev];
    set({ settled: next });
    persist(next);
  },

  unsettle: (workspaceId) => {
    const prev = get().settled;
    if (!prev.some((e) => e.id === workspaceId)) return;
    const next = prev.filter((e) => e.id !== workspaceId);
    set({ settled: next });
    persist(next);
  },

  setFilter: (projectPath) => set({ filter: projectPath }),

  prune: (validIds) => {
    const prev = get().settled;
    const next = prev.filter((e) => validIds.has(e.id));
    if (next.length === prev.length) return;
    set({ settled: next });
    persist(next);
  },
}));

/** Test-only: reset the module-level load memoization + store state. */
export function __resetSidebarInboxStoreForTests(): void {
  loadPromise = null;
  useSidebarInboxStore.setState({
    loaded: false,
    settled: [],
    filter: null,
  });
}
