export type KeybindCategory =
  | "general"
  | "search"
  | "workspaces"
  | "tabs"
  | "panes"
  | "terminal";

export interface KeybindEntry {
  id: string;
  label: string;
  category: KeybindCategory;
  defaultKeys: string;
  description?: string;
  /** "terminal" means handled inside xterm, not at the window level */
  when?: "always" | "terminal";
}

export const KEYBIND_REGISTRY: readonly KeybindEntry[] = [
  // ── General ──
  { id: "commandPalette", label: "Command palette", category: "general", defaultKeys: "Ctrl+K" },
  { id: "toggleSidebar", label: "Toggle sidebar", category: "general", defaultKeys: "Ctrl+B" },
  { id: "openSettings", label: "Open settings", category: "general", defaultKeys: "Ctrl+," },
  { id: "closeOverlay", label: "Close overlay", category: "general", defaultKeys: "Escape", description: "Close settings, search, or command palette" },

  // ── Search ──
  { id: "fileSearch", label: "Find file by name", category: "search", defaultKeys: "Ctrl+P" },
  { id: "contentSearch", label: "Search in files", category: "search", defaultKeys: "Ctrl+Shift+F" },

  // ── Workspaces ──
  { id: "nextWorkspace", label: "Next workspace", category: "workspaces", defaultKeys: "Ctrl+]" },
  { id: "prevWorkspace", label: "Previous workspace", category: "workspaces", defaultKeys: "Ctrl+[" },
  { id: "runDevCommand", label: "Run dev command", category: "workspaces", defaultKeys: "Ctrl+Shift+G" },

  // ── Tabs ──
  { id: "newTab", label: "New terminal tab", category: "tabs", defaultKeys: "Ctrl+T" },
  { id: "closeTab", label: "Close tab", category: "tabs", defaultKeys: "Ctrl+W" },
  { id: "switchTab1", label: "Switch to tab 1", category: "tabs", defaultKeys: "Ctrl+1" },
  { id: "switchTab2", label: "Switch to tab 2", category: "tabs", defaultKeys: "Ctrl+2" },
  { id: "switchTab3", label: "Switch to tab 3", category: "tabs", defaultKeys: "Ctrl+3" },
  { id: "switchTab4", label: "Switch to tab 4", category: "tabs", defaultKeys: "Ctrl+4" },
  { id: "switchTab5", label: "Switch to tab 5", category: "tabs", defaultKeys: "Ctrl+5" },
  { id: "switchTab6", label: "Switch to tab 6", category: "tabs", defaultKeys: "Ctrl+6" },
  { id: "switchTab7", label: "Switch to tab 7", category: "tabs", defaultKeys: "Ctrl+7" },
  { id: "switchTab8", label: "Switch to tab 8", category: "tabs", defaultKeys: "Ctrl+8" },
  { id: "switchTab9", label: "Switch to tab 9", category: "tabs", defaultKeys: "Ctrl+9" },

  // ── Panes ──
  { id: "splitPaneRight", label: "Split pane right", category: "panes", defaultKeys: "Ctrl+Shift+D" },
  { id: "closePane", label: "Close pane", category: "panes", defaultKeys: "Ctrl+Shift+W" },

  // ── Terminal (handled inside xterm) ──
  { id: "copySelection", label: "Copy selection", category: "terminal", defaultKeys: "Ctrl+Shift+C", when: "terminal" },
  { id: "pasteTerminal", label: "Paste", category: "terminal", defaultKeys: "Ctrl+Shift+V", when: "terminal" },
  { id: "backwardKillWord", label: "Backward kill word", category: "terminal", defaultKeys: "Ctrl+Backspace", when: "terminal" },
] as const;

const registryById = new Map(KEYBIND_REGISTRY.map((e) => [e.id, e]));

export function getRegistryEntry(id: string): KeybindEntry | undefined {
  return registryById.get(id);
}

/** All categories in display order */
export const KEYBIND_CATEGORIES: readonly KeybindCategory[] = [
  "general",
  "search",
  "workspaces",
  "tabs",
  "panes",
  "terminal",
];

/** Human-readable category labels */
export const CATEGORY_LABELS: Record<KeybindCategory, string> = {
  general: "General",
  search: "Search",
  workspaces: "Workspaces",
  tabs: "Tabs",
  panes: "Panes",
  terminal: "Terminal",
};
