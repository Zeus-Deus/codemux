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
}

export const usePluginsStore = create<PluginsState>()(() => ({
  records: {},
  decisions: {},
  decisionsLoaded: false,
  disabledByEnv: false,
  paused: false,
  pausedReason: null,
  rootPath: null,
}));

const handles = new Map<string, PluginHandle>();

/** The user's choice if any, else the plugin's own default. */
export function pluginActive(id: string, defaultEnabled: boolean): boolean {
  const { decisions } = usePluginsStore.getState();
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

export function hasHandle(id: string): boolean {
  return handles.has(id);
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
  usePluginsStore.setState((s) => ({ decisions: { ...s.decisions, [id]: enabled } }));
  void persistDecision(id, enabled).catch((error) =>
    console.error(`[plugins] could not save the decision for ${id}`, error),
  );
  const handle = handles.get(id);
  if (!handle) return;
  if (enabled) {
    await handle.activate();
  } else {
    handle.deactivate();
    patchPlugin(id, { status: "disabled", error: undefined });
  }
}

/** Deactivate every live plugin (kill switch, safe-mode pause, tests). */
export function deactivateAll(filter: (r: PluginRecord) => boolean = () => true): void {
  for (const record of Object.values(usePluginsStore.getState().records)) {
    if (!filter(record)) continue;
    handles.get(record.id)?.deactivate();
    if (record.status === "loaded") patchPlugin(record.id, { status: "disabled" });
  }
}

/** Test-only reset. */
export function resetPluginsStoreForTests(): void {
  handles.clear();
  persistDecision = async () => {};
  usePluginsStore.setState({
    records: {},
    decisions: {},
    decisionsLoaded: false,
    disabledByEnv: false,
    paused: false,
    pausedReason: null,
    rootPath: null,
  });
}
