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
/** Persisted UI-state key for the plugin layout the Projects view uses: a
 *  native tree (`<installationId>/tree:<treeId>`) or declared view
 *  (`<installationId>/<viewId>`), or empty for the built-in grouping. Kept while the
 *  plugin is off or unavailable, so turning it back on restores it. */
export const SIDEBAR_PLUGIN_LAYOUT_UI_STATE_KEY = "sidebar.pluginLayout";
/** Persisted UI-state key for the person's folds in plugin trees: a JSON
 *  array of section keys (`+key` records an unfold against the default). */
export const SIDEBAR_PLUGIN_TREE_FOLDS_UI_STATE_KEY = "sidebar.pluginTree.folds";

export const DEFAULT_SIDEBAR_VIEW: SidebarView = "activity";

export function parseSidebarView(raw: string | null | undefined): SidebarView {
  return raw === "projects" ? "projects" : DEFAULT_SIDEBAR_VIEW;
}

function parseStringList(raw: string | null | undefined): string[] {
  if (!raw) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter((entry): entry is string => typeof entry === "string") : [];
  } catch {
    return [];
  }
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
  /** The plugin layout the Projects view uses, or null for the built-in
   *  grouping. Only a choice: the view falls back to the built-in grouping
   *  whenever the plugin cannot serve it. */
  pluginLayout: string | null;
  /** Fold entries for plugin tree sections. */
  pluginTreeFolds: string[];
  load: () => Promise<void>;
  setView: (view: SidebarView) => void;
  toggleView: () => void;
  setFolded: (projectPath: string, folded: boolean) => void;
  /** Choose a plugin layout (switching to Projects), or null for the built-in grouping. */
  setPluginLayout: (key: string | null) => void;
  setPluginTreeFolds: (folds: string[]) => void;
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
  pluginLayout: null,
  pluginTreeFolds: [],

  load: () => {
    if (loadPromise) return loadPromise;
    loadPromise = Promise.all([
      dbGetUiState(SIDEBAR_VIEW_UI_STATE_KEY).catch(() => null),
      dbGetUiState(SIDEBAR_FOLDED_PROJECTS_UI_STATE_KEY).catch(() => null),
      dbGetUiState(SIDEBAR_PLUGIN_LAYOUT_UI_STATE_KEY).catch(() => null),
      dbGetUiState(SIDEBAR_PLUGIN_TREE_FOLDS_UI_STATE_KEY).catch(() => null),
    ]).then(([rawView, rawFolded, rawLayout, rawTreeFolds]) => {
      // A choice made before the read resolved (a click or the shortcut in
      // the first frames) wins over the stored value.
      if (get().loaded) return;
      set({
        loaded: true,
        view: parseSidebarView(rawView),
        folded: parseFolded(rawFolded),
        pluginLayout: rawLayout ? rawLayout : null,
        pluginTreeFolds: parseStringList(rawTreeFolds),
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

  setPluginLayout: (key) => {
    set({ pluginLayout: key, loaded: true, ...(key !== null ? { view: "projects" as const } : {}) });
    dbSetUiState(SIDEBAR_PLUGIN_LAYOUT_UI_STATE_KEY, key ?? "").catch(console.error);
    if (key !== null) dbSetUiState(SIDEBAR_VIEW_UI_STATE_KEY, "projects").catch(console.error);
  },

  setPluginTreeFolds: (folds) => {
    set({ pluginTreeFolds: folds });
    dbSetUiState(SIDEBAR_PLUGIN_TREE_FOLDS_UI_STATE_KEY, JSON.stringify(folds)).catch(console.error);
  },
}));

/** Test-only: reset the load memoization and store state. */
export function __resetSidebarViewStoreForTests(): void {
  loadPromise = null;
  useSidebarViewStore.setState({
    loaded: false,
    view: DEFAULT_SIDEBAR_VIEW,
    folded: {},
    pluginLayout: null,
    pluginTreeFolds: [],
  });
}
