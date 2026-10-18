import { create } from "zustand";
import { dbGetUiState, dbSetUiState } from "@/tauri/commands";

/** Which list the expanded sidebar shows.
 *
 *  - `activity`: the flat workspace inbox (`SidebarInbox`) — the default, and
 *    exactly what the sidebar was before the switch existed.
 *  - `projects`: chats on top, then one foldable group per project
 *    (`SidebarProjectsView`). */
export type SidebarView = "activity" | "projects";

/** Persisted UI-state key for the chosen view. */
export const SIDEBAR_VIEW_UI_STATE_KEY = "sidebar.view";
/** Persisted UI-state key for the folded project groups in the Projects view:
 *  a JSON array of project paths. One blob rather than one key per project so
 *  the view reads its whole fold state in a single round trip. */
export const SIDEBAR_FOLDED_PROJECTS_UI_STATE_KEY = "sidebar.projects.folded";

export const DEFAULT_SIDEBAR_VIEW: SidebarView = "activity";

export function parseSidebarView(raw: string | null | undefined): SidebarView {
  return raw === "projects" ? "projects" : DEFAULT_SIDEBAR_VIEW;
}

function parseFolded(raw: string | null | undefined): Record<string, true> {
  if (!raw) return {};
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return {};
    const out: Record<string, true> = {};
    for (const path of parsed) {
      if (typeof path === "string" && path) out[path] = true;
    }
    return out;
  } catch {
    return {};
  }
}

interface SidebarViewStore {
  /** True once the persisted choice has been read (or failed to read). The
   *  list waits for it so a user who left the sidebar on Projects never sees
   *  the inbox mount and unmount on launch. */
  loaded: boolean;
  view: SidebarView;
  /** Project paths whose group is folded in the Projects view. */
  folded: Record<string, true>;
  load: () => Promise<void>;
  setView: (view: SidebarView) => void;
  toggleView: () => void;
  setFolded: (projectPath: string, folded: boolean) => void;
}

let loadPromise: Promise<void> | null = null;

function persistFolded(folded: Record<string, true>): void {
  dbSetUiState(
    SIDEBAR_FOLDED_PROJECTS_UI_STATE_KEY,
    JSON.stringify(Object.keys(folded)),
  ).catch(console.error);
}

export const useSidebarViewStore = create<SidebarViewStore>((set, get) => ({
  loaded: false,
  view: DEFAULT_SIDEBAR_VIEW,
  folded: {},

  load: () => {
    if (loadPromise) return loadPromise;
    loadPromise = Promise.all([
      dbGetUiState(SIDEBAR_VIEW_UI_STATE_KEY).catch(() => null),
      dbGetUiState(SIDEBAR_FOLDED_PROJECTS_UI_STATE_KEY).catch(() => null),
    ]).then(([rawView, rawFolded]) => {
      // A choice made before the read resolved (a click or the shortcut in
      // the first frames) wins over the stored value.
      if (get().loaded) return;
      set({
        loaded: true,
        view: parseSidebarView(rawView),
        folded: parseFolded(rawFolded),
      });
    });
    return loadPromise;
  },

  setView: (view) => {
    if (get().view === view && get().loaded) return;
    set({ view, loaded: true });
    dbSetUiState(SIDEBAR_VIEW_UI_STATE_KEY, view).catch(console.error);
  },

  toggleView: () => {
    get().setView(get().view === "activity" ? "projects" : "activity");
  },

  setFolded: (projectPath, folded) => {
    const current = get().folded;
    if ((current[projectPath] === true) === folded) return;
    const next = { ...current };
    if (folded) next[projectPath] = true;
    else delete next[projectPath];
    set({ folded: next });
    persistFolded(next);
  },
}));

/** Test-only: reset the load memoization and store state. */
export function __resetSidebarViewStoreForTests(): void {
  loadPromise = null;
  useSidebarViewStore.setState({
    loaded: false,
    view: DEFAULT_SIDEBAR_VIEW,
    folded: {},
  });
}
