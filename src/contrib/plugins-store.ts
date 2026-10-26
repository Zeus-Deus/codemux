/**
 * Plugin inventory: one record per plugin the app knows about (bundled and
 * disk), plus the user's enable decisions. Loaders publish records with
 * activate/deactivate handles, so toggling is live and never needs a reload.
 */
import { create } from "zustand";

export type PluginKind = "bundled" | "disk";
export type PluginStatus = "disabled" | "loading" | "error" | "loaded";
/** Official: bundled or a catalog official. Unverified: other installs. Local: your own folder. */
export type PluginTier = "official" | "unverified" | "local";

export interface PluginRecord {
  id: string;
  name: string;
  kind: PluginKind;
  status: PluginStatus;
  tier: PluginTier;
  description?: string;
  version?: string;
  error?: string;
  /** Absolute plugin.js path (disk plugins). */
  file?: string;
  /** Install provenance (disk plugins with a marker). */
  origin?: { source: string; repo: string; sha: string; catalogId?: string };
  /** A row that is informational only (shadowed copy, duplicate id). */
  inert?: boolean;
  /** What the switch shows when the user hasn't chosen. */
  defaultEnabled?: boolean;
}

export interface PluginHandle {
  activate: () => void | Promise<void>;
  deactivate: () => void;
}

interface PluginsState {
  records: Record<string, PluginRecord>;
  /** Explicit user choices; absence means "use the plugin's default". */
  decisions: Record<string, boolean>;
  /** Decisions have been read from the native registry. */
  decisionsLoaded: boolean;
  /** `CODEMUX_DISABLE_PLUGINS=1` was set at launch. */
  disabledByEnv: boolean;
  /** Safe mode: non-bundled plugins are paused for this launch. */
  paused: boolean;
  pausedReason: string | null;
  rootPath: string | null;
  /** Toggles not yet saved natively: null while saving, else the save error. */
  unsavedDecisions: Record<string, string | null>;
  /** Plugins whose installation is being removed or replaced right now. */
  changing: Record<string, true>;
}

export const usePluginsStore = create<PluginsState>()(() => ({
  records: {},
  decisions: {},
  decisionsLoaded: false,
  disabledByEnv: false,
  paused: false,
  pausedReason: null,
  rootPath: null,
  unsavedDecisions: {},
  changing: {},
}));

const handles = new Map<string, PluginHandle>();

/** The user's choice if any, else the plugin's own default. */
export function pluginActive(id: string, defaultEnabled: boolean): boolean {
  const { decisions, changing } = usePluginsStore.getState();
  // Mid remove/fresh install: nothing of it may run until that settles.
  if (changing[id]) return false;
  return id in decisions ? decisions[id] : defaultEnabled;
}

export function publishPlugin(record: PluginRecord, handle?: PluginHandle): void {
  usePluginsStore.setState((s) => ({ records: { ...s.records, [record.id]: record } }));
  if (handle) handles.set(record.id, handle);
}

export function patchPlugin(id: string, patch: Partial<PluginRecord>): void {
  const current = usePluginsStore.getState().records[id];
  if (!current) return;
  usePluginsStore.setState((s) => ({ records: { ...s.records, [id]: { ...current, ...patch } } }));
}

export function dropPlugin(id: string): void {
  const { [id]: _dropped, ...rest } = usePluginsStore.getState().records;
  usePluginsStore.setState({ records: rest });
  handles.delete(id);
}

/**
 * Decisions the user made that the native registry doesn't have yet: still
 * being written, or the write failed. They override the native copy until a
 * write succeeds or the user changes them, so a failed "off" is never undone
 * by the next rescan.
 */
const unsaved = new Map<string, boolean>();
/** Per plugin: the number of the latest toggle. Only that toggle's save may
 *  clear the override or report an error. */
const latestAttempt = new Map<string, number>();
let attempts = 0;
/** Per plugin: the save chain, so writes land in the order they were made
 *  and an older write can never be the last one persisted. */
const writes = new Map<string, Promise<void>>();

/**
 * Remove, or a fresh install, replaces the installation behind `id`. While
 * that native operation runs:
 * - the plugin can't be activated (a rescan in the window, e.g. a file save
 *   or Reload plugins, sees it as off), and its switch can't be toggled;
 * - every save already queued for it has landed BEFORE this resolves, so no
 *   save of the old installation can land after the native step.
 * The old installation's choices (including a failed "off") stay
 * authoritative until `commit()`, which drops them atomically once the
 * native step succeeded. `rollback()` keeps them; rescan afterwards so the
 * live state matches the switch again.
 */
export async function beginInstallationChange(
  id: string,
): Promise<{ commit: () => void; rollback: () => void }> {
  usePluginsStore.setState((s) => ({ changing: { ...s.changing, [id]: true } }));
  // Drain: wait until no save for this plugin is queued or in flight.
  for (let chain = writes.get(id); chain; chain = writes.get(id)) {
    await chain.catch(() => undefined);
    if (writes.get(id) === chain) writes.delete(id);
  }
  const end = () =>
    usePluginsStore.setState((s) => {
      const { [id]: _done, ...rest } = s.changing;
      return { changing: rest };
    });
  let settled = false;
  return {
    commit: () => {
      if (settled) return;
      settled = true;
      unsaved.delete(id);
      latestAttempt.delete(id);
      usePluginsStore.setState((s) => {
        const { [id]: _gone, ...rest } = s.unsavedDecisions;
        const { [id]: _old, ...decisions } = s.decisions;
        return { unsavedDecisions: rest, decisions };
      });
      end();
    },
    rollback: () => {
      if (settled) return;
      settled = true;
      end();
    },
  };
}

/**
 * Adopt the native registry's decisions. Every rescan does this first:
 * install and remove write decisions natively, so a stale renderer copy
 * would otherwise decide (a removed plugin's old "on" would turn on a
 * reinstall the user chose to leave off). Toggles still being saved win.
 */
export function replaceDecisions(decisions: Record<string, boolean>): void {
  usePluginsStore.setState({ decisions: { ...decisions, ...Object.fromEntries(unsaved) } });
}

/** Native persistence for decisions; installed by the loader at boot. */
let persistDecision: (id: string, enabled: boolean) => Promise<void> = async () => {};

export function setDecisionPersistence(fn: (id: string, enabled: boolean) => Promise<void>): void {
  persistDecision = fn;
}

/**
 * Live toggle from Settings → Plugins. This is the ONLY writer of decisions;
 * the SDK exposes no verb for it, so one plugin can't switch another.
 */
export async function setPluginEnabled(id: string, enabled: boolean): Promise<void> {
  // Its installation is being replaced or removed; the switch is disabled.
  if (usePluginsStore.getState().changing[id]) return;
  usePluginsStore.setState((s) => ({
    decisions: { ...s.decisions, [id]: enabled },
    unsavedDecisions: { ...s.unsavedDecisions, [id]: null },
  }));
  unsaved.set(id, enabled);
  const attempt = ++attempts;
  latestAttempt.set(id, attempt);
  const isLatest = () => latestAttempt.get(id) === attempt;
  const write = (writes.get(id) ?? Promise.resolve()).then(() => persistDecision(id, enabled));
  const tail = write.then(
    () => {
      if (!isLatest()) return;
      unsaved.delete(id);
      usePluginsStore.setState((s) => {
        const { [id]: _done, ...rest } = s.unsavedDecisions;
        return { unsavedDecisions: rest };
      });
    },
    (error) => {
      console.error(`[plugins] could not save the decision for ${id}`, error);
      if (!isLatest()) return;
      // Kept in `unsaved`: this choice still governs this session.
      usePluginsStore.setState((s) => ({
        unsavedDecisions: { ...s.unsavedDecisions, [id]: error instanceof Error ? error.message : String(error) },
      }));
    },
  );
  writes.set(id, tail);
  void tail.finally(() => {
    if (writes.get(id) === tail) writes.delete(id);
  });
  const handle = handles.get(id);
  if (!handle) return;
  const { disabledByEnv, paused, records } = usePluginsStore.getState();
  // Kill switch and safe mode hold code back even when a switch is flipped;
  // the decision is saved and applies on the next normal start.
  if (enabled && (disabledByEnv || (paused && records[id]?.kind !== "bundled"))) return;
  if (enabled) {
    await handle.activate();
  } else {
    handle.deactivate();
    patchPlugin(id, { status: "disabled", error: undefined });
  }
}

/** Test-only reset. */
export function resetPluginsStoreForTests(): void {
  handles.clear();
  unsaved.clear();
  latestAttempt.clear();
  writes.clear();
  persistDecision = async () => {};
  usePluginsStore.setState({
    records: {},
    decisions: {},
    decisionsLoaded: false,
    disabledByEnv: false,
    paused: false,
    pausedReason: null,
    rootPath: null,
    unsavedDecisions: {},
    changing: {},
  });
}
