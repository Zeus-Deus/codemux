/**
 * The UI's one seam to isolated platform plugins. Every native call goes
 * through `plugins_platform_request({ method, params })`, which the desktop
 * serves directly and the web-remote shim forwards; components never invoke
 * the backend themselves. The catalogue is read lazily on first use, so an
 * app without the platform (or one whose plugin store fails) only loses
 * plugin UI, never startup or core work.
 */
import { useEffect, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { useShallow } from "zustand/react/shallow";
import type {
  PluginCardReference,
  PluginCatalogue,
  PluginChanged,
  PluginEventBacklog,
  PluginEventBacklogPolicy,
  PluginGrants,
  PluginInstallation,
  PluginInvokeInput,
  PluginInvokeResult,
  PluginRepairWorkspace,
  PluginSettingValue,
  PluginSource,
  PluginTreeSnapshot,
} from "./contract";
import { pluginFailure } from "./model";
import { readPluginCatalogue, readPluginInstallation, readPluginInvokeResult } from "./wire";

export const PLATFORM_REQUEST_COMMAND = "plugins_platform_request";
export const PLATFORM_CHANGED_EVENT = "plugins_platform_changed";
export const PLATFORM_CATALOGUE_CHANGED_EVENT = "plugins_platform_catalogue_changed";

/** Rejects with a `PluginFailure` ({ reason, message }), whatever the transport threw. */
export async function platformRequest<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
  try {
    return await invoke<T>(PLATFORM_REQUEST_COMMAND, { method, params });
  } catch (error) {
    throw pluginFailure(error);
  }
}

// ── catalogue store ───────────────────────────────────────────────────

interface PlatformPluginsState {
  catalogue: PluginCatalogue | null;
  /** The first read finished, successfully or not. */
  loaded: boolean;
  /** The last read failed; the previous catalogue (if any) is kept. */
  error: string | null;
  /** How many times each `installation\0generation\0topic` changed since this client subscribed. */
  revisions: ReadonlyMap<string, number>;
}

export const usePlatformPluginsStore = create<PlatformPluginsState>()(() => ({
  catalogue: null,
  loaded: false,
  error: null,
  revisions: new Map(),
}));

let started = false;
let reading: Promise<void> | null = null;
let again = false;
/** The last catalogue as received, so an identical re-read keeps every consumer's references. */
let lastCatalogueJson: string | null = null;

/** Generation is part of the key: a refreshed installation's signals never reach old consumers. */
export const pluginChangeKey = (change: Pick<PluginChanged, "installationId" | "generation" | "topic">) =>
  `${change.installationId}\0${change.generation}\0${change.topic}`;

export const pluginTreeTopic = (treeId: string) => `tree:${treeId}`;
export const pluginViewTopic = (viewId: string) => `view:${viewId}`;
export const pluginStatusTopic = (statusId: string) => `status:${statusId}`;

/** Reads the catalogue again; concurrent asks collapse into one more read. */
export function refreshPlatformCatalogue(): Promise<void> {
  if (reading) {
    again = true;
    return reading;
  }
  reading = (async () => {
    do {
      again = false;
      try {
        const raw = await platformRequest<unknown>("catalogue");
        const catalogue = readPluginCatalogue(raw);
        if (catalogue === null) {
          throw { reason: "invalid_result", message: "Plugins are unavailable in this version of Codemux." };
        }
        // A focus or signal that changed nothing must not re-render every plugin surface.
        const json = JSON.stringify(catalogue);
        const state = usePlatformPluginsStore.getState();
        if (json === lastCatalogueJson && state.catalogue !== null && state.loaded && state.error === null) continue;
        lastCatalogueJson = json;
        usePlatformPluginsStore.setState({
          catalogue,
          loaded: true,
          error: null,
        });
      } catch (error) {
        usePlatformPluginsStore.setState({ loaded: true, error: pluginFailure(error).message });
      }
    } while (again);
  })().finally(() => {
    reading = null;
  });
  return reading;
}

/** Starts the catalogue read and change subscriptions once per app. Safe to call anywhere. */
export function ensurePlatformPlugins(): void {
  if (started) return;
  started = true;
  void refreshPlatformCatalogue();
  void listen(PLATFORM_CATALOGUE_CHANGED_EVENT, () => void refreshPlatformCatalogue()).catch(() => {});
  void listen<PluginChanged>(PLATFORM_CHANGED_EVENT, (event) => {
    const change = event.payload;
    if (!change || typeof change.topic !== "string") return;
    usePlatformPluginsStore.setState((state) => {
      const key = pluginChangeKey(change);
      return { revisions: new Map(state.revisions).set(key, (state.revisions.get(key) ?? 0) + 1) };
    });
  }).catch(() => {});
}

/** Puts a record the host just returned into the catalogue before the next read. */
function upsert(raw: unknown): PluginInstallation {
  const installation = readPluginInstallation(raw);
  if (installation === null) {
    void refreshPlatformCatalogue();
    throw { reason: "invalid_result", message: "The plugin host returned an unreadable record." };
  }
  lastCatalogueJson = null;
  usePlatformPluginsStore.setState((state) => {
    if (state.catalogue === null) return state;
    const installations = state.catalogue.installations.some((entry) => entry.installationId === installation.installationId)
      ? state.catalogue.installations.map((entry) =>
          entry.installationId === installation.installationId ? installation : entry,
        )
      : [...state.catalogue.installations, installation];
    return { catalogue: { ...state.catalogue, installations } };
  });
  void refreshPlatformCatalogue();
  return installation;
}

/**
 * The catalogue and its load state. Change signals (`revisions`) are left
 * out, so a plugin's frequent signals re-render only the surface that
 * listens for that topic.
 */
export function usePlatformCatalogue(): Pick<PlatformPluginsState, "catalogue" | "loaded" | "error"> {
  useEffect(ensurePlatformPlugins, []);
  return usePlatformPluginsStore(
    useShallow((state) => ({ catalogue: state.catalogue, loaded: state.loaded, error: state.error })),
  );
}

/** The current change count for one topic of one generation, or undefined before any signal. */
export function usePlatformChangeRevision(
  change: Pick<PluginChanged, "installationId" | "generation" | "topic"> | null,
): number | undefined {
  const key = change === null ? null : pluginChangeKey(change);
  return usePlatformPluginsStore((state) => (key === null ? undefined : state.revisions.get(key)));
}

/** `source` of every contribution registered for an isolated platform plugin. */
export const PLATFORM_CONTRIBUTION_SOURCE = "platform";

/**
 * A platform plugin's display name for host chrome that labels contributions,
 * whose ids start with @ followed by the installation id (two installations may share a
 * manifest id when they come from different sources).
 */
export function platformPluginLabel(contributionOwner: string): string {
  const installationId = contributionOwner.startsWith("@") ? contributionOwner.slice(1) : contributionOwner;
  const installation = usePlatformPluginsStore
    .getState()
    .catalogue?.installations.find((entry) => entry.installationId === installationId);
  return installation?.manifest.name ?? "Plugin";
}

/** Imperative read for menus that capture their entries when they open. */
export const readPlatformCatalogue = () => usePlatformPluginsStore.getState().catalogue;

// ── requests ──────────────────────────────────────────────────────────

export const platformInvoke = async (input: PluginInvokeInput): Promise<PluginInvokeResult> =>
  readPluginInvokeResult(await platformRequest<unknown>("invoke", { ...input }));

async function viewDocument(method: string, params: Record<string, unknown>): Promise<{ html: string }> {
  const result = await platformRequest<unknown>(method, params);
  const html = typeof result === "object" && result !== null ? (result as { html?: unknown }).html : undefined;
  if (typeof html !== "string") throw { reason: "invalid_result", message: "The plugin host returned no view." };
  return { html };
}

export const platformReadView = (input: { installationId: string; generation: string; viewId: string }) =>
  viewDocument("readView", input);

export const platformReadCardView = (input: { installationId: string; generation: string; card: PluginCardReference }) =>
  viewDocument("readCardView", input);

export const platformReadTree = (input: { installationId: string; generation: string; treeId: string; operationId: string }) =>
  platformRequest<PluginTreeSnapshot>("readTree", input);

export const platformInstall = async (source: PluginSource) =>
  upsert(await platformRequest<unknown>("install", { source }));

export const platformConsent = async (input: {
  installationId: string;
  digest: string;
  grants: PluginGrants;
  eventBacklog?: PluginEventBacklogPolicy;
}) => upsert(await platformRequest<unknown>("consent", input));

export const platformSetEnabled = async (input: {
  installationId: string;
  enabled: boolean;
  eventBacklog?: PluginEventBacklogPolicy;
}) => upsert(await platformRequest<unknown>("setEnabled", input));

export const platformResume = async (input: { installationId: string; eventBacklog?: PluginEventBacklogPolicy }) =>
  upsert(await platformRequest<unknown>("resume", input));

export const platformRefresh = async (installationId: string) =>
  upsert(await platformRequest<unknown>("refresh", { installationId }));

export const platformDiscardPendingUpdate = async (installationId: string) =>
  upsert(await platformRequest<unknown>("discardPendingUpdate", { installationId }));

export const platformUpdateSettings = async (installationId: string, values: Record<string, PluginSettingValue>) =>
  upsert(await platformRequest<unknown>("updateSettings", { installationId, values }));

export async function platformRemove(installationId: string, deleteData: boolean): Promise<void> {
  await platformRequest<unknown>("remove", { installationId, ...(deleteData ? { deleteData: true } : {}) });
  usePlatformPluginsStore.setState((state) =>
    state.catalogue === null
      ? state
      : {
          catalogue: {
            ...state.catalogue,
            installations: state.catalogue.installations.filter((entry) => entry.installationId !== installationId),
          },
        },
  );
  void refreshPlatformCatalogue();
}

/** Missed runs; an unreadable answer is an error, never "nothing missed". */
export async function platformBacklog(installationId: string): Promise<PluginEventBacklog> {
  const raw = await platformRequest<unknown>("backlog", { installationId });
  const value = (typeof raw === "object" && raw !== null ? raw : {}) as Record<string, unknown>;
  const count = (key: string) =>
    typeof value[key] === "number" && Number.isSafeInteger(value[key]) && (value[key] as number) >= 0
      ? (value[key] as number)
      : null;
  const [total, after, through] = [count("count"), count("afterSequence"), count("throughSequence")];
  if (total === null || after === null || through === null || typeof value.requiresChoice !== "boolean") {
    throw { reason: "invalid_result", message: "The plugin host returned an unreadable missed-runs answer." };
  }
  return { count: total, afterSequence: after, throughSequence: through, requiresChoice: value.requiresChoice };
}

export const platformPrepareRepair = async (installationId: string): Promise<PluginRepairWorkspace> => {
  const raw = await platformRequest<unknown>("prepareRepair", { installationId });
  void refreshPlatformCatalogue();
  const value = (typeof raw === "object" && raw !== null ? raw : {}) as Record<string, unknown>;
  const fields = ["workspaceId", "directory", "baseDigest", "baseGeneration", "draft"] as const;
  if (!fields.every((field) => typeof value[field] === "string")) {
    throw { reason: "invalid_result", message: "The plugin host returned no repair folder." };
  }
  return Object.fromEntries(fields.map((field) => [field, value[field]])) as unknown as PluginRepairWorkspace;
};

export const platformStageRepair = async (input: { installationId: string; workspaceId: string; baseDigest: string }) =>
  upsert(await platformRequest<unknown>("stageRepair", input));

export const platformStagePrevious = async (input: { installationId: string; expectedDigest: string }) =>
  upsert(
    await platformRequest<unknown>("stagePrevious", { ...input, acknowledgeCurrentData: true }),
  );

// ── visibility ────────────────────────────────────────────────────────

const subscribeVisibility = (listener: () => void) => {
  document.addEventListener("visibilitychange", listener);
  return () => document.removeEventListener("visibilitychange", listener);
};
const documentVisible = () => document.visibilityState !== "hidden";

/** Whether this window is in front; status polling stops behind it. */
export const useDocumentVisible = () => useSyncExternalStore(subscribeVisibility, documentVisible, () => true);

/** Test-only reset. */
export function resetPlatformPluginsForTests(state: Partial<PlatformPluginsState> = {}): void {
  started = true;
  reading = null;
  again = false;
  lastCatalogueJson = null;
  usePlatformPluginsStore.setState({ catalogue: null, loaded: false, error: null, revisions: new Map(), ...state });
}
