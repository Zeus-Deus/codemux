import { create } from "zustand";
import type { UserSettings } from "@/tauri/types";
import {
  getSyncedSettings,
  updateSyncedSettings,
  updateSetting as updateSettingCmd,
  resetSyncedSettings,
} from "@/tauri/commands";

const DEFAULT_SETTINGS: UserSettings = {
  appearance: {
    theme: "default",
    custom_themes: [],
    shell_font: null,
    typography_mode: "simple",
    interface_font_family: null,
    interface_font_size: 16,
    conversation_font_family: null,
    conversation_font_size: 14,
    code_font_family: null,
    code_font_size: 13,
    terminal_font_family: null,
    terminal_font_size: 13,
    show_resource_monitor: true,
  },
  editor: { default_ide: null },
  terminal: { scrollback_limit: 10_000, cursor_style: "bar" },
  git: { default_base_branch: "main" },
  source_control: { custom_hosts: {}, open_pr_links_in_browser: false },
  keyboard: { shortcuts: {} },
  notifications: { sound_enabled: true, desktop_enabled: true },
  file_tree: { show_hidden_files: false },
  session_restore: { enabled: true, scrollback_lines: 10_000, max_total_mb: 100 },
  agent_chat: { checkpoints_enabled: false, background_browser_desktop_viewport: true },
  browser: { default_viewport: null },
};

/** Outstanding writes that did not reach your account:
 *  `saved-locally` is on this machine and syncs on the next refresh;
 *  `failed` was not saved anywhere. `null` once all edits have synced. */
export type SyncIssue = "saved-locally" | "failed";

export interface SyncedSettingsState {
  settings: UserSettings;
  isLoading: boolean;
  isSyncing: boolean;
  syncIssue: SyncIssue | null;
}

interface SyncedSettingsActions {
  loadSettings: () => Promise<void>;
  updateSettings: (settings: UserSettings) => Promise<void>;
  updateSetting: (section: string, key: string, value: unknown) => Promise<void>;
  resetSettings: () => Promise<void>;
  applySettingsFromEvent: (settings: UserSettings, synced?: boolean) => void;
  /** Capture before a remote account refresh; its result only applies while
   * no newer local write or session replacement has happened. */
  remoteReconcileToken: () => SettingsOperationToken;
  finishRemoteReconcile: (token: SettingsOperationToken) => void;
  reconcileRemoteSettings: (
    settings: UserSettings,
    token: SettingsOperationToken,
    synced?: boolean,
  ) => boolean;
  replaceSessionSettings: (settings: UserSettings) => void;
}

type SyncedSettingsStore = SyncedSettingsState & SyncedSettingsActions;

export { DEFAULT_SETTINGS };

const EMPTY_CUSTOM_THEMES: unknown[] = [];
/** Stable identity so the selector doesn't re-render on every read. */
const EMPTY_CUSTOM_HOSTS: Record<string, string> = {};

// Each optimistic write increments this. Async responses only apply if
// the generation matches, preventing stale backend responses from
// reverting a newer optimistic update.
let _settingsGen = 0;
// Account/session replacements advance this independently of local writes.
// In-flight counts are keyed by it so a slow request from the account that
// just signed out cannot suppress the next account's settings response.
let _sessionGen = 0;

interface SettingsOperationToken {
  sessionGeneration: number;
  settingsGeneration: number;
}

const _inflightWrites = new Map<number, number>();
const _inflightRemoteReconciles = new Map<number, number>();
// A PATCH only confirms its own field. A full snapshot covers all edits up
// to its captured generation, including edits whose PATCH is still pending.
const _writeIssues = new Map<string, { generation: number; issue: SyncIssue }>();
const _writeStatusGenerations = new Map<string, number>();
const FULL_WRITE = "*";

function writeOutcome(
  token: SettingsOperationToken,
  scope: string,
  issue: SyncIssue | null,
  set: (partial: Partial<SyncedSettingsState>) => void,
) {
  if (!sameSession(token)) return;
  const generation = token.settingsGeneration;
  if (generation < Math.max(
    _writeStatusGenerations.get(scope) ?? 0,
    _writeStatusGenerations.get(FULL_WRITE) ?? 0,
  )) return;
  _writeStatusGenerations.set(scope, generation);
  if (scope === FULL_WRITE && issue !== "failed") {
    for (const [field, status] of _writeIssues) {
      if (status.generation <= generation) _writeIssues.delete(field);
    }
  } else {
    _writeIssues.delete(scope);
  }
  if (issue) _writeIssues.set(scope, { generation, issue });
  set({ syncIssue: [..._writeIssues.values()].some((s) => s.issue === "failed")
    ? "failed" : _writeIssues.size > 0 ? "saved-locally" : null });
}

function clearQueuedIssues(set: (partial: Partial<SyncedSettingsState>) => void) {
  for (const [field, status] of _writeIssues) {
    if (status.issue === "saved-locally") _writeIssues.delete(field);
  }
  set({ syncIssue: _writeIssues.size > 0 ? "failed" : null });
}

function incrementInflight(map: Map<number, number>, sessionGeneration: number) {
  map.set(sessionGeneration, (map.get(sessionGeneration) ?? 0) + 1);
}

function decrementInflight(map: Map<number, number>, sessionGeneration: number) {
  const remaining = (map.get(sessionGeneration) ?? 0) - 1;
  if (remaining > 0) map.set(sessionGeneration, remaining);
  else map.delete(sessionGeneration);
}

function currentInflight(map: Map<number, number>): number {
  return map.get(_sessionGen) ?? 0;
}

function beginWrite(): SettingsOperationToken {
  const token = {
    sessionGeneration: _sessionGen,
    settingsGeneration: ++_settingsGen,
  };
  incrementInflight(_inflightWrites, token.sessionGeneration);
  return token;
}

function isCurrent(token: SettingsOperationToken): boolean {
  return (
    token.sessionGeneration === _sessionGen &&
    token.settingsGeneration === _settingsGen
  );
}

function finishWrite(
  token: SettingsOperationToken,
  set: (partial: Partial<SyncedSettingsState>) => void,
) {
  decrementInflight(_inflightWrites, token.sessionGeneration);
  if (
    token.sessionGeneration === _sessionGen &&
    currentInflight(_inflightWrites) === 0
  ) {
    set({ isSyncing: false });
  }
}

function sameSession(token: SettingsOperationToken): boolean {
  return token.sessionGeneration === _sessionGen;
}

export const useSyncedSettingsStore = create<SyncedSettingsStore>()((set, get) => ({
  settings: DEFAULT_SETTINGS,
  isLoading: true,
  isSyncing: false,
  syncIssue: null,

  loadSettings: async () => {
    const token = { sessionGeneration: _sessionGen, settingsGeneration: _settingsGen };
    set({ isLoading: true });
    try {
      const result = await getSyncedSettings();
      if (isCurrent(token) && currentInflight(_inflightWrites) === 0) {
        set({ settings: result.settings, isLoading: false });
        if (result.synced) clearQueuedIssues(set);
      }
    } catch {
      if (sameSession(token)) set({ isLoading: false });
    }
  },

  updateSettings: async (settings) => {
    const token = beginWrite();
    set({ settings, isSyncing: true });
    try {
      const result = await updateSyncedSettings(settings);
      if (isCurrent(token)) set({ settings: result.settings });
      writeOutcome(token, FULL_WRITE, result.synced ? null : "saved-locally", set);
    } catch {
      // Keep the optimistic value. A full write already saves locally when
      // the server is unreachable, so an error here was not saved at all.
      writeOutcome(token, FULL_WRITE, "failed", set);
    } finally {
      finishWrite(token, set);
    }
  },

  updateSetting: async (section, key, value) => {
    const token = beginWrite();
    const scope = `${section}.${key}`;
    // Optimistic update — apply locally first
    set((s) => {
      const json = JSON.parse(JSON.stringify(s.settings)) as Record<string, Record<string, unknown>>;
      if (json[section]) {
        json[section][key] = value;
      }
      return { settings: json as unknown as UserSettings, isSyncing: true };
    });
    try {
      const result = await updateSettingCmd(section, key, value);
      const saved = result.settings;
      writeOutcome(token, scope, result.synced ? null : "saved-locally", set);
      if (isCurrent(token)) {
        // Re-apply our intended value on top of the server response.
        // The server PATCH deep-merges nested objects, so sending
        // e.g. { shortcuts: {} } is a no-op from the server's
        // perspective. Force-set the exact field we wrote.
        const patched = JSON.parse(JSON.stringify(saved)) as Record<string, Record<string, unknown>>;
        if (patched[section]) patched[section][key] = value;
        const corrected = patched as unknown as UserSettings;
        set({ settings: corrected });

        // If the server response didn't match our intent (deep-merge
        // semantics), do a background full PUT to correct the server.
        const serverVal = JSON.stringify((saved as unknown as Record<string, Record<string, unknown>>)[section]?.[key]);
        const intendedVal = JSON.stringify(value);
        if (serverVal !== intendedVal) {
          // Keep the correction in this write's lifetime so persistence
          // failures and offline saves remain visible to the user.
          const correctedResult = await updateSyncedSettings(corrected);
          writeOutcome(token, FULL_WRITE, correctedResult.synced ? null : "saved-locally", set);
        }
      }
    } catch {
      // A single-field write has no offline path in the backend: it fails
      // without touching the local cache, so the edit would vanish on the
      // next refresh. A full write does save locally and syncs later, so
      // fall back to it with everything the user currently sees.
      if (!sameSession(token)) return;
      const fallbackToken = { ...token, settingsGeneration: _settingsGen };
      try {
        const result = await updateSyncedSettings(get().settings);
        writeOutcome(fallbackToken, FULL_WRITE, result.synced ? null : "saved-locally", set);
      } catch {
        writeOutcome(token, scope, "failed", set);
      }
    } finally {
      finishWrite(token, set);
    }
  },

  resetSettings: async () => {
    const token = beginWrite();
    set({ settings: DEFAULT_SETTINGS, isSyncing: true });
    try {
      const result = await resetSyncedSettings();
      if (isCurrent(token)) set({ settings: result.settings });
      writeOutcome(token, FULL_WRITE, result.synced ? null : "saved-locally", set);
    } catch {
      // Keep the optimistic defaults. The backend owns offline persistence and
      // the next explicit refresh will reconcile them.
      writeOutcome(token, FULL_WRITE, "failed", set);
    } finally {
      finishWrite(token, set);
    }
  },

  applySettingsFromEvent: (settings, synced) => {
    // Skip events while local writes are in flight — the async response
    // path handles those with gen-checks. Only apply events from
    // external sources (other devices, server push).
    if (
      currentInflight(_inflightWrites) > 0 ||
      currentInflight(_inflightRemoteReconciles) > 0
    ) {
      return;
    }
    set({ settings });
    if (synced) clearQueuedIssues(set);
    else if (synced === false) {
      writeOutcome({ sessionGeneration: _sessionGen, settingsGeneration: _settingsGen }, FULL_WRITE, "saved-locally", set);
    }
  },

  remoteReconcileToken: () => {
    const token = {
      sessionGeneration: _sessionGen,
      settingsGeneration: _settingsGen,
    };
    incrementInflight(
      _inflightRemoteReconciles,
      token.sessionGeneration,
    );
    return token;
  },

  finishRemoteReconcile: (token) => {
    decrementInflight(
      _inflightRemoteReconciles,
      token.sessionGeneration,
    );
  },

  reconcileRemoteSettings: (settings, token, synced = false) => {
    if (!isCurrent(token) || currentInflight(_inflightWrites) > 0) {
      return false;
    }
    set({ settings, isLoading: false });
    if (synced) clearQueuedIssues(set);
    return true;
  },

  replaceSessionSettings: (settings) => {
    // Invalidate responses belonging to the previous account even when its
    // writes happened to finish before the replacement arrived.
    _sessionGen += 1;
    _settingsGen += 1;
    _inflightWrites.clear();
    _inflightRemoteReconciles.clear();
    _writeIssues.clear();
    _writeStatusGenerations.clear();
    set({ settings, isLoading: false, isSyncing: false, syncIssue: null });
  },
}));

// ── React hook selectors (trigger re-renders on specific value change) ──

export const selectAppearanceTheme = (s: SyncedSettingsState): string =>
  s.settings.appearance.theme || "default";

export const selectCustomThemes = (s: SyncedSettingsState): unknown[] =>
  s.settings.appearance.custom_themes ?? EMPTY_CUSTOM_THEMES;

export const selectTerminalCursorStyle = (s: SyncedSettingsState): string =>
  s.settings.terminal.cursor_style;

export const selectDefaultEditor = (s: SyncedSettingsState): string =>
  s.settings.editor.default_ide ?? "";

export const selectDefaultBaseBranch = (s: SyncedSettingsState): string =>
  s.settings.git.default_base_branch;

export const selectNotificationSoundEnabled = (s: SyncedSettingsState): boolean =>
  s.settings.notifications.sound_enabled;

export const selectDesktopNotificationsEnabled = (s: SyncedSettingsState): boolean =>
  s.settings.notifications.desktop_enabled;

/** Self-hosted instances the user has classified, host → product. */
export const selectCustomHosts = (s: SyncedSettingsState): Record<string, string> =>
  s.settings.source_control?.custom_hosts ?? EMPTY_CUSTOM_HOSTS;

/** True when host pull-request links should keep going to the browser
 *  instead of opening the Pull Requests page. */
export const selectOpenPrLinksInBrowser = (s: SyncedSettingsState): boolean =>
  s.settings.source_control?.open_pr_links_in_browser ?? false;

export const selectKeyboardShortcuts = (s: SyncedSettingsState): Record<string, string> =>
  s.settings.keyboard.shortcuts;

export const selectShowHiddenFiles = (s: SyncedSettingsState): boolean =>
  s.settings.file_tree.show_hidden_files;

export const selectShowResourceMonitor = (s: SyncedSettingsState): boolean =>
  s.settings.appearance.show_resource_monitor;

export const selectAgentCheckpointsEnabled = (s: SyncedSettingsState): boolean =>
  s.settings.agent_chat?.checkpoints_enabled ?? false;

export const selectBackgroundBrowserDesktopViewport = (s: SyncedSettingsState): boolean =>
  s.settings.agent_chat?.background_browser_desktop_viewport ?? true;

/** Raw `browser.default_viewport` spec string (`"2560x1440"` etc.), or
 *  null for the built-in baseline. Returns the primitive so zustand's
 *  reference equality works — parse with `parseViewportString`. */
export const selectBrowserDefaultViewport = (s: SyncedSettingsState): string | null =>
  s.settings.browser?.default_viewport ?? null;

/** Parse a `"WxH"` viewport string into dimensions. Returns null for
 *  anything else (unset, preset names, garbage from another device) so
 *  callers fall back to their own baseline — mirrors the lenient
 *  degrade-to-default behavior of `browser_viewport::resolve_default`
 *  on the Rust side. */
export function parseViewportString(raw: string | null): { width: number; height: number } | null {
  if (!raw) return null;
  const m = /^(\d{2,4})x(\d{2,4})$/.exec(raw.trim().toLowerCase());
  if (!m) return null;
  const width = Number(m[1]);
  const height = Number(m[2]);
  if (width < 10 || height < 10 || width > 7680 || height > 7680) return null;
  return { width, height };
}
