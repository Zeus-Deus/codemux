import { create } from "zustand";
import { persist } from "zustand/middleware";
import {
  RIGHT_PANEL_MAX_STORED_WIDTH,
  RIGHT_PANEL_MIN_WIDTH,
} from "@/lib/right-panel-width";
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
 * `rightPanelPanes`, never grows a tab, and resolves to the first open pane
 * if there is one (see `right-panel.tsx`), so activating it is also the
 * honest "just open the panel to whatever it was showing" request.
 */
export const RIGHT_PANEL_EMPTY = "empty";

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

interface UIStore {
  rightPanelTabs: Record<string, RightPanelTab | null>;
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
  showFileSearch: boolean;
  showContentSearch: boolean;
  pendingWorkspaces: PendingWorkspace[];
  lastSelectedAgentId: string | null;
  /** Last model + reasoning choice per agent family (`claude`, `codex`,
   *  `opencode`, `gemini`), so reopening the New Workspace dialog
   *  restores the user's pick instead of resetting to Default. */
  lastModelSelections: Record<string, ModelSelection>;
  showCommandPalette: boolean;
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
  setShowFileSearch: (show: boolean, target?: "editor" | "right-panel") => void;
  setShowContentSearch: (show: boolean) => void;
  addPendingWorkspace: (pw: PendingWorkspace) => void;
  removePendingWorkspace: (id: string) => void;
  failPendingWorkspace: (id: string, error: string) => void;
  setLastSelectedAgentId: (id: string | null) => void;
  setLastModelSelection: (family: string, selection: ModelSelection) => void;
  setShowCommandPalette: (show: boolean) => void;
  toggleCommandPalette: () => void;
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
      showFileSearch: false,
      showContentSearch: false,
      pendingWorkspaces: [],
      lastSelectedAgentId: null,
      lastModelSelections: {},
      showCommandPalette: false,
      showCloneDialog: false,
      showNewProjectScreen: false,
      onboardingProjectDir: null,
      hasSeenOnboarding: false,
      sidebarToggleFn: null,
      expandProjectRequest: null,
      subagentEnterRequest: null,

      getRightPanelTab: (workspaceId) => get().rightPanelTabs[workspaceId] ?? null,

      setRightPanelTab: (workspaceId, tab) =>
        set((s) => {
          const tabs = { ...s.rightPanelTabs, [workspaceId]: tab };
          // Collapsing always drops full-expand: a maximized panel that is
          // no longer on screen would leave the workspace column at zero
          // width with nothing beside it.
          if (tab === null) {
            return { rightPanelTabs: tabs, rightPanelMaximized: false };
          }
          // The picker sentinel is a view state, not a pane — it must never
          // join the deck or clear a dismissal.
          if (tab === RIGHT_PANEL_EMPTY) return { rightPanelTabs: tabs };
          const list = s.rightPanelPanes[workspaceId] ?? DEFAULT_RIGHT_PANEL_PANES;
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
        }),

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
