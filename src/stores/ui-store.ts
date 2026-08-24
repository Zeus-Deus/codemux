import { create } from "zustand";
import { persist } from "zustand/middleware";
import {
  RIGHT_PANEL_MAX_STORED_WIDTH,
  RIGHT_PANEL_MIN_WIDTH,
} from "@/lib/right-panel-width";
import { useAppStore } from "@/stores/app-store";
import { undockBrowserFromRightPanel } from "@/tauri/commands";
import type { ModelSelection, PendingWorkspace } from "@/tauri/types";

/** Panes the right-panel deck can host that aren't tied to a file path. */
export type RightPanelCorePane =
  | "changes"
  | "files"
  | "review"
  | "orchestration"
  | "tasks"
  | "subagents"
  | "diff"
  | "browser";

/**
 * The panel is open but no pane is selected — the surface picker.
 *
 * `rightPanelTabs[ws] === null` has always meant "collapsed", so "open and
 * empty" needed a third value rather than a second boolean: every caller
 * already reads `!== null` as *is the panel on screen*, and they all stay
 * correct. It is deliberately **not** a registry pane — it never enters
 * `rightPanelPanes`, never grows a tab, and resolves to the pane the panel
 * was collapsed from (`rightPanelLastTabs`) when that pane is still open,
 * else to the first open pane if there is one (see `right-panel.tsx`), so
 * activating it is also the honest "just open the panel to whatever it was
 * showing" request.
 */
export const RIGHT_PANEL_EMPTY = "empty";

/** What the Theme Studio should open on — one of its two tabs, or an
 *  existing custom theme to reopen. */
export type ThemeStudioRequest =
  | { mode: "generate" }
  | { mode: "import" }
  | { editThemeId: string };

/** A pane id in the right-panel deck. `doc:<absolute path>` panes are
 *  opened per file, so the id carries its own payload — that keeps the
 *  open-pane list a plain, persistable string array. */
export type RightPanelTab =
  | RightPanelCorePane
  | `doc:${string}`
  | typeof RIGHT_PANEL_EMPTY;

/** The deck a workspace starts with. Matches the three panes that were
 *  always-present tabs before the deck existed, so an upgrade is a no-op
 *  for anyone who never opens the `+` menu. */
export const DEFAULT_RIGHT_PANEL_PANES: readonly RightPanelTab[] = [
  "files",
  "changes",
  "review",
];

function withPane(
  list: readonly RightPanelTab[],
  pane: RightPanelTab,
): RightPanelTab[] {
  return list.includes(pane) ? [...list] : [...list, pane];
}

/** Is this workspace's agent browser session currently hosted by the deck? */
function isBrowserDocked(workspaceId: string): boolean {
  return (
    useAppStore
      .getState()
      .appState?.agent_browser_sessions?.some(
        (abs) => abs.workspace_id === workspaceId && abs.right_panel_docked,
      ) === true
  );
}

interface UIStore {
  rightPanelTabs: Record<string, RightPanelTab | null>;
  /** The pane each workspace was showing when its panel was last collapsed.
   *  Opening on {@link RIGHT_PANEL_EMPTY} comes back here (if the pane is
   *  still in the deck) instead of landing on the first tab. */
  rightPanelLastTabs: Record<string, RightPanelTab>;
  /** Ordered open panes per workspace. Absent ⇒ {@link DEFAULT_RIGHT_PANEL_PANES}. */
  rightPanelPanes: Record<string, RightPanelTab[]>;
  /** Availability-gated panes (tasks, orchestration, subagents) auto-open
   *  when their data appears. Closing one records it here so the auto-open
   *  effect doesn't immediately put it back. Reopening clears the record. */
  rightPanelDismissedPanes: Record<string, RightPanelTab[]>;
  rightPanelWidth: number;
  /** Measured width of the row the panel shares with the workspace content
   *  (`workspace-main.tsx` owns the measurement). Runtime-only, never
   *  persisted, `0` until first layout.
   *
   *  This is the one fact the panel's real width limit depends on, so it is
   *  published rather than re-derived: `rightPanelWidth` is the width the
   *  *user asked for* and can exceed what currently fits, and anything that
   *  has to line up with the panel's rendered edge (the title bar's
   *  floating band) or offer its maximum (the expand toggle) needs this to
   *  clamp against. See `@/lib/right-panel-width`. */
  rightPanelRowWidth: number;
  /**
   * Full-expand: the panel takes the entire content row and the workspace
   * column collapses to zero width beside it (it stays mounted — terminals
   * and scroll positions survive).
   *
   * Deliberately **not persisted** and deliberately **not a width**. The
   * stored `rightPanelWidth` is left untouched while this is on, so
   * restoring is exact and free — there is no previous-width bookkeeping to
   * get out of sync. Closing the panel clears it (see `setRightPanelTab`),
   * so the app can never boot into a maximized panel over a hidden chat.
   */
  rightPanelMaximized: boolean;
  /** Where the file-search dialog should open its pick: a main-area editor
   *  tab (the historic behavior) or a right-panel doc pane. */
  fileSearchTarget: "editor" | "right-panel";
  showNewWorkspaceDialog: boolean;
  newWorkspaceProjectDir: string | null;
  showSettings: boolean;
  settingsSection: string | null;
  /** Set to request creating a new preset and opening its editor. The
   *  Presets settings view consumes and clears it. Not persisted. */
  pendingPresetCreate: boolean;
  showAutomations: boolean;
  showWorkspacesOverview: boolean;
  showPullRequests: boolean;
  /** A pull request the page should select as it opens — set by the
   *  palette, consumed and cleared by the page. Not persisted. */
  pendingPrSelection: { projectRoot: string; number: number } | null;
  /** Rows the badge has already shown the user. Keys, not a count, so a
   *  poll that re-reports the same review request doesn't re-raise a
   *  badge that was just cleared. Transient by design: a fresh session
   *  should tell you what is waiting. */
  prBadgeSeen: string[];
  /** Workspace targeted by the app-level rename dialog. Transient: menus
   *  unmount as soon as their item is chosen, so the request lives here and
   *  the dialog can stay mounted above every workspace surface. */
  renameWorkspaceId: string | null;
  showFileSearch: boolean;
  showContentSearch: boolean;
  pendingWorkspaces: PendingWorkspace[];
  lastSelectedAgentId: string | null;
  /** Last model + reasoning choice per agent family (`claude`, `codex`,
   *  `opencode`, `gemini`), so reopening the New Workspace dialog
   *  restores the user's pick instead of resetting to Default. */
  lastModelSelections: Record<string, ModelSelection>;
  showCommandPalette: boolean;
  /**
   * Seed text the palette should open with, consumed once on mount.
   *
   * The theme picker lives behind a query (`⌘K → theme`), so the surfaces
   * that mean "let me change the theme" — Settings ▸ Appearance's Change
   * button — have to hand the palette the word rather than reimplement the
   * list. Transient; never persisted.
   */
  commandPaletteQuery: string | null;
  /**
   * Theme Studio request, or null while it is closed. `"generate"` and
   * `"import"` open the two tabs the palette's last two rows point at; a
   * `{ editThemeId }` request reopens a saved custom theme.
   * Transient; never persisted.
   */
  themeStudio: ThemeStudioRequest | null;
  showCloneDialog: boolean;
  showNewProjectScreen: boolean;
  onboardingProjectDir: string | null;
  hasSeenOnboarding: boolean;
  /** Callback ref set by AppShell after SidebarProvider mounts */
  sidebarToggleFn: (() => void) | null;
  /** One-shot request to expand a specific project group by path. Set by the
   *  "Needs you" strip before it scrolls to a blocked row (a row inside a
   *  collapsed group isn't in the DOM, so the jump would otherwise no-op).
   *  The matching {@link SidebarProjectGroup} consumes it — expands + persists
   *  — and clears it. Transient (not persisted). */
  expandProjectRequest: string | null;
  /** One-shot request to drill into a subagent in the chat transcript,
   *  raised by the right panel's Subagents pane. The matching
   *  {@link AgentChatPane} consumes it (the drill-in is that pane's own
   *  state) and clears it. Transient — never persisted. */
  subagentEnterRequest: {
    threadId: string;
    subagentId: string;
    nonce: number;
  } | null;

  getRightPanelTab: (workspaceId: string) => RightPanelTab | null;
  /** Activate a pane. Also opens it (and clears any dismissal) so every
   *  legacy caller — the titlebar toggle, the command palette, the
   *  in-thread "Open tasks"/"Open orchestration" links — lands on a tab
   *  that's actually in the strip. */
  setRightPanelTab: (workspaceId: string, tab: RightPanelTab | null) => void;
  /** Hide the panel. The one collapse path — every caller (titlebar cluster,
   *  the panel's own close button, the keybind, the legacy tab bar) routes
   *  here, and `setRightPanelTab(ws, null)` delegates to it, so the "a
   *  collapsed panel is not a surface" rule cannot be bypassed. */
  collapseRightPanel: (workspaceId: string) => void;
  toggleRightPanel: (workspaceId: string, tab: RightPanelTab) => void;
  getRightPanelPanes: (workspaceId: string) => RightPanelTab[];
  /** Open a pane in the background (no focus change) — used by the
   *  availability auto-open for tasks/orchestration/subagents. */
  addRightPanelPane: (workspaceId: string, pane: RightPanelTab) => void;
  closeRightPanelPane: (workspaceId: string, pane: RightPanelTab) => void;
  setRightPanelWidth: (width: number) => void;
  setRightPanelRowWidth: (width: number) => void;
  /** Toggle full-expand. No-op while the panel is collapsed. */
  toggleRightPanelMaximized: (workspaceId: string) => void;
  setShowNewWorkspaceDialog: (show: boolean, projectDir?: string | null) => void;
  setShowSettings: (show: boolean, section?: string | null) => void;
  /** Open Settings ▸ Presets and request creating a new preset. */
  requestNewPreset: () => void;
  /** Clear the pending-create request after the settings view handles it. */
  clearPendingPresetCreate: () => void;
  setShowAutomations: (show: boolean) => void;
  setShowWorkspacesOverview: (show: boolean) => void;
  /** Open the Pull Requests page, optionally on a given pull request. */
  setShowPullRequests: (
    show: boolean,
    select?: { projectRoot: string; number: number } | null,
  ) => void;
  clearPendingPrSelection: () => void;
  /** Mark what the badge was counting as seen — called when the page
   *  opens, with the keys that were on it. */
  markPrBadgeSeen: (keys: string[]) => void;
  requestRenameWorkspace: (workspaceId: string) => void;
  closeRenameWorkspace: () => void;
  setShowFileSearch: (show: boolean, target?: "editor" | "right-panel") => void;
  setShowContentSearch: (show: boolean) => void;
  addPendingWorkspace: (pw: PendingWorkspace) => void;
  removePendingWorkspace: (id: string) => void;
  failPendingWorkspace: (id: string, error: string) => void;
  setLastSelectedAgentId: (id: string | null) => void;
  setLastModelSelection: (family: string, selection: ModelSelection) => void;
  setShowCommandPalette: (show: boolean) => void;
  toggleCommandPalette: () => void;
  /** Open the palette pre-filled with `query` (see {@link commandPaletteQuery}). */
  openCommandPaletteWith: (query: string) => void;
  /** Read-and-clear the seed query, so reopening the palette starts empty. */
  takeCommandPaletteQuery: () => string | null;
  openThemeStudio: (request: ThemeStudioRequest) => void;
  closeThemeStudio: () => void;
  setShowCloneDialog: (show: boolean) => void;
  setShowNewProjectScreen: (show: boolean) => void;
  setOnboardingProjectDir: (dir: string | null) => void;
  setSidebarToggleFn: (fn: (() => void) | null) => void;
  /** Ask the project group at `projectPath` to expand (see
   *  {@link expandProjectRequest}). */
  requestExpandProject: (projectPath: string) => void;
  /** Clear the pending expand request, but only if it still targets
   *  `projectPath` — so a newer request for a different group isn't clobbered
   *  by a stale consumer. */
  clearExpandProjectRequest: (projectPath: string) => void;
  requestEnterSubagent: (threadId: string, subagentId: string) => void;
  clearSubagentEnterRequest: () => void;
}

export const useUIStore = create<UIStore>()(
  persist(
    (set, get) => ({
      rightPanelTabs: {},
      rightPanelLastTabs: {},
      rightPanelPanes: {},
      rightPanelDismissedPanes: {},
      rightPanelWidth: 320,
      rightPanelRowWidth: 0,
      rightPanelMaximized: false,
      fileSearchTarget: "editor",
      showNewWorkspaceDialog: false,
      newWorkspaceProjectDir: null,
      showSettings: false,
      settingsSection: null,
      pendingPresetCreate: false,
      showAutomations: false,
      showWorkspacesOverview: false,
      showPullRequests: false,
      pendingPrSelection: null,
      prBadgeSeen: [],
      renameWorkspaceId: null,
      showFileSearch: false,
      showContentSearch: false,
      pendingWorkspaces: [],
      lastSelectedAgentId: null,
      lastModelSelections: {},
      showCommandPalette: false,
      commandPaletteQuery: null,
      themeStudio: null,
      showCloneDialog: false,
      showNewProjectScreen: false,
      onboardingProjectDir: null,
      hasSeenOnboarding: false,
      sidebarToggleFn: null,
      expandProjectRequest: null,
      subagentEnterRequest: null,

      getRightPanelTab: (workspaceId) => get().rightPanelTabs[workspaceId] ?? null,

      setRightPanelTab: (workspaceId, tab) => {
        // Collapsing is its own action (it has a backend consequence, see
        // `collapseRightPanel`); route it there rather than duplicating the
        // rule at every call site that happens to pass `null`.
        if (tab === null) {
          get().collapseRightPanel(workspaceId);
          return;
        }
        set((s) => {
          const list = s.rightPanelPanes[workspaceId] ?? DEFAULT_RIGHT_PANEL_PANES;
          // The picker sentinel is "open to whatever it was showing": a
          // collapsed panel comes back on the pane it was collapsed from,
          // provided that pane is still in the deck. It is a view state,
          // not a pane — it must never join the deck or clear a dismissal.
          if (tab === RIGHT_PANEL_EMPTY) {
            const last = s.rightPanelLastTabs[workspaceId];
            const restored =
              s.rightPanelTabs[workspaceId] == null && last && list.includes(last)
                ? last
                : tab;
            return { rightPanelTabs: { ...s.rightPanelTabs, [workspaceId]: restored } };
          }
          const tabs = { ...s.rightPanelTabs, [workspaceId]: tab };
          const dismissed = s.rightPanelDismissedPanes[workspaceId] ?? [];
          return {
            rightPanelTabs: tabs,
            rightPanelPanes: {
              ...s.rightPanelPanes,
              [workspaceId]: withPane(list, tab),
            },
            rightPanelDismissedPanes: {
              ...s.rightPanelDismissedPanes,
              [workspaceId]: dismissed.filter((p) => p !== tab),
            },
          };
        });
      },

      collapseRightPanel: (workspaceId) => {
        // A collapsed panel is not a surface. Leaving the session docked
        // would tell the backend the user can see a browser they can't: the
        // pane gate would keep believing it is surfaced, so the agent would
        // neither split a pane for it nor raise the background chip, and the
        // browser would be invisible *and* unrevealable until the panel came
        // back. `dismissed: false` — collapsing the panel is not "close this
        // browser", so the agent may still surface it, the tab stays in the
        // deck, and re-opening the panel re-docks it.
        if (isBrowserDocked(workspaceId)) {
          undockBrowserFromRightPanel(workspaceId, false).catch(console.error);
        }
        set((s) => {
          const active = s.rightPanelTabs[workspaceId] ?? null;
          return {
            rightPanelTabs: { ...s.rightPanelTabs, [workspaceId]: null },
            rightPanelLastTabs:
              active === null || active === RIGHT_PANEL_EMPTY
                ? s.rightPanelLastTabs
                : { ...s.rightPanelLastTabs, [workspaceId]: active },
            // Collapsing always drops full-expand: a maximized panel that is
            // no longer on screen would leave the workspace column at zero
            // width with nothing beside it.
            rightPanelMaximized: false,
          };
        });
      },

      toggleRightPanel: (workspaceId, tab) => {
        const current = get().rightPanelTabs[workspaceId] ?? null;
        get().setRightPanelTab(workspaceId, current === tab ? null : tab);
      },

      getRightPanelPanes: (workspaceId) =>
        get().rightPanelPanes[workspaceId] ?? [...DEFAULT_RIGHT_PANEL_PANES],

      addRightPanelPane: (workspaceId, pane) =>
        set((s) => {
          const list = s.rightPanelPanes[workspaceId] ?? DEFAULT_RIGHT_PANEL_PANES;
          if (list.includes(pane)) return s;
          return {
            rightPanelPanes: {
              ...s.rightPanelPanes,
              [workspaceId]: [...list, pane],
            },
          };
        }),

      // Closing the active pane hands focus to its neighbour; closing the
      // last one lands on the surface picker ({@link RIGHT_PANEL_EMPTY})
      // rather than collapsing the whole panel. Closing a tab and losing the
      // column it lived in are different requests — the panel toggle in the
      // titlebar is still the way to collapse.
      closeRightPanelPane: (workspaceId, pane) =>
        set((s) => {
          const list = s.rightPanelPanes[workspaceId] ?? DEFAULT_RIGHT_PANEL_PANES;
          const index = list.indexOf(pane);
          if (index === -1) return s;
          const next = list.filter((p) => p !== pane);
          const active = s.rightPanelTabs[workspaceId] ?? null;
          const dismissed = s.rightPanelDismissedPanes[workspaceId] ?? [];
          return {
            rightPanelPanes: { ...s.rightPanelPanes, [workspaceId]: next },
            rightPanelDismissedPanes: {
              ...s.rightPanelDismissedPanes,
              [workspaceId]: dismissed.includes(pane)
                ? dismissed
                : [...dismissed, pane],
            },
            rightPanelTabs: {
              ...s.rightPanelTabs,
              [workspaceId]:
                active === pane
                  ? (next[Math.min(index, next.length - 1)] ?? RIGHT_PANEL_EMPTY)
                  : active,
            },
          };
        }),

      // Stores the width the user asked for, bounded only for sanity. The
      // limit that depends on the current window — "at most 75% of the
      // content row, and always leave room for the chat" — is applied where
      // that row can actually be measured (`workspace-main.tsx`), so a
      // panel sized on a wide monitor survives a session on a narrow one
      // instead of being permanently clipped down to it.
      setRightPanelWidth: (width) =>
        set({
          rightPanelWidth: Math.max(
            RIGHT_PANEL_MIN_WIDTH,
            Math.min(RIGHT_PANEL_MAX_STORED_WIDTH, width),
          ),
        }),

      setRightPanelRowWidth: (width) =>
        set((state) =>
          state.rightPanelRowWidth === width
            ? state
            : { rightPanelRowWidth: width },
        ),

      toggleRightPanelMaximized: (workspaceId) =>
        set((state) => {
          const open = (state.rightPanelTabs[workspaceId] ?? null) !== null;
          if (!open) return state;
          return { rightPanelMaximized: !state.rightPanelMaximized };
        }),

      setShowNewWorkspaceDialog: (show, projectDir = null) =>
        set({ showNewWorkspaceDialog: show, newWorkspaceProjectDir: show ? (projectDir ?? null) : null }),

      setShowSettings: (show, section = null) => set({ showSettings: show, settingsSection: show ? (section ?? null) : null }),
      requestNewPreset: () =>
        set({
          showSettings: true,
          settingsSection: "presets",
          pendingPresetCreate: true,
        }),
      clearPendingPresetCreate: () => set({ pendingPresetCreate: false }),
      setShowAutomations: (show) => set({ showAutomations: show }),
      setShowWorkspacesOverview: (show) => set({ showWorkspacesOverview: show }),
      setShowPullRequests: (show, select = null) =>
        set({ showPullRequests: show, pendingPrSelection: show ? select : null }),
      clearPendingPrSelection: () => set({ pendingPrSelection: null }),
      markPrBadgeSeen: (keys) =>
        set((state) => ({
          prBadgeSeen: [...new Set([...state.prBadgeSeen, ...keys])],
        })),
      requestRenameWorkspace: (workspaceId) =>
        set({ renameWorkspaceId: workspaceId }),
      closeRenameWorkspace: () => set({ renameWorkspaceId: null }),
      setShowFileSearch: (show, target = "editor") =>
        set({ showFileSearch: show, fileSearchTarget: show ? target : "editor" }),
      setShowContentSearch: (show) => set({ showContentSearch: show }),

      addPendingWorkspace: (pw) =>
        set((s) => ({ pendingWorkspaces: [...s.pendingWorkspaces, pw] })),

      removePendingWorkspace: (id) =>
        set((s) => ({
          pendingWorkspaces: s.pendingWorkspaces.filter((pw) => pw.id !== id),
        })),

      failPendingWorkspace: (id, error) =>
        set((s) => ({
          pendingWorkspaces: s.pendingWorkspaces.map((pw) =>
            pw.id === id ? { ...pw, status: "failed" as const, errorMessage: error } : pw,
          ),
        })),

      setLastSelectedAgentId: (id) => set({ lastSelectedAgentId: id }),

      setLastModelSelection: (family, selection) =>
        set((s) => ({
          lastModelSelections: { ...s.lastModelSelections, [family]: selection },
        })),

      setShowCommandPalette: (show) => set({ showCommandPalette: show }),
      toggleCommandPalette: () => set((s) => ({ showCommandPalette: !s.showCommandPalette })),

      // Deliberately does NOT close Settings. Settings is a full-screen
      // destination, so this used to have to leave it for the palette to
      // mount at all — which meant Change ejected you from Appearance, and
      // Esc out of the studio landed you on the home screen. `SettingsView`
      // now mounts its own `CommandPalette`, so the palette (and the studio
      // opened from it) layers over Settings and Esc returns you to the page
      // you were on.
      openCommandPaletteWith: (query) =>
        set({ commandPaletteQuery: query, showCommandPalette: true }),
      takeCommandPaletteQuery: () => {
        const { commandPaletteQuery } = get();
        if (commandPaletteQuery !== null) set({ commandPaletteQuery: null });
        return commandPaletteQuery;
      },

      openThemeStudio: (request) => set({ themeStudio: request }),
      closeThemeStudio: () => set({ themeStudio: null }),

      setShowCloneDialog: (show) => set({ showCloneDialog: show }),

      setShowNewProjectScreen: (show) => set({ showNewProjectScreen: show }),

      setOnboardingProjectDir: (dir) =>
        set((s) =>
          dir === null
            ? { onboardingProjectDir: null, hasSeenOnboarding: true }
            : { onboardingProjectDir: dir, hasSeenOnboarding: s.hasSeenOnboarding },
        ),

      setSidebarToggleFn: (fn) => set({ sidebarToggleFn: fn }),

      requestExpandProject: (projectPath) =>
        set({ expandProjectRequest: projectPath }),

      clearExpandProjectRequest: (projectPath) =>
        set((s) =>
          s.expandProjectRequest === projectPath
            ? { expandProjectRequest: null }
            : s,
        ),

      requestEnterSubagent: (threadId, subagentId) =>
        set((s) => ({
          subagentEnterRequest: {
            threadId,
            subagentId,
            nonce: (s.subagentEnterRequest?.nonce ?? 0) + 1,
          },
        })),

      clearSubagentEnterRequest: () => set({ subagentEnterRequest: null }),
    }),
    {
      name: "codemux-ui",
      version: 1,
      partialize: (state) => ({
        rightPanelTabs: state.rightPanelTabs,
        rightPanelLastTabs: state.rightPanelLastTabs,
        rightPanelPanes: state.rightPanelPanes,
        rightPanelDismissedPanes: state.rightPanelDismissedPanes,
        rightPanelWidth: state.rightPanelWidth,
        lastSelectedAgentId: state.lastSelectedAgentId,
        lastModelSelections: state.lastModelSelections,
        hasSeenOnboarding: state.hasSeenOnboarding,
      }),
      // v0 → v1: the right-panel tab id `"pr"` was renamed to `"review"`
      // when the panel itself was renamed (Phase 3). Rewrite any persisted
      // values so users keep their active tab on upgrade instead of having
      // it silently fall back to the default.
      migrate: (persistedState, version) => {
        if (version >= 1) return persistedState;
        const state = persistedState as { rightPanelTabs?: Record<string, string | null> };
        if (state?.rightPanelTabs) {
          const migrated: Record<string, string | null> = {};
          for (const [wsId, tab] of Object.entries(state.rightPanelTabs)) {
            migrated[wsId] = tab === "pr" ? "review" : tab;
          }
          state.rightPanelTabs = migrated;
        }
        return state;
      },
    },
  ),
);
