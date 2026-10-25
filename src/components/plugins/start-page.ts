/**
 * A plugin page this device opens when Codemux starts. Kept on the device in
 * localStorage, never synced. It names the installation, which the host
 * derives from the plugin id *and* its source, so another publisher's plugin
 * with the same id never inherits it; reinstalling from the same source keeps
 * the same installation. Anything missing, failed or not allowed starts the
 * usual way, so a broken plugin never stands between the person and their
 * work.
 */
import type { PluginInstallation } from "./contract";
import { isPluginActive } from "./model";

export interface PluginStartPage {
  /** Null only in a preference saved before installations were recorded. */
  readonly installationId: string | null;
  /** The manifest id, kept to resolve an older preference without guessing. */
  readonly pluginId: string;
  readonly viewId: string;
}

const START_PAGE_KEY = "codemux:plugins:start-page";

export function readPluginStartPage(storage: Pick<Storage, "getItem"> | null = safeStorage()): PluginStartPage | null {
  try {
    const raw = storage?.getItem(START_PAGE_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return null;
    const { installationId, pluginId, viewId } = parsed as Record<string, unknown>;
    if (typeof pluginId !== "string" || typeof viewId !== "string") return null;
    return { installationId: typeof installationId === "string" ? installationId : null, pluginId, viewId };
  } catch {
    return null;
  }
}

export function writePluginStartPage(
  value: PluginStartPage | null,
  storage: Pick<Storage, "setItem" | "removeItem"> | null = safeStorage(),
): void {
  try {
    if (value === null) storage?.removeItem(START_PAGE_KEY);
    else storage?.setItem(START_PAGE_KEY, JSON.stringify(value));
  } catch {
    // Storage full or blocked: the preference just doesn't stick.
  }
}

function safeStorage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

/** The preference for one installation's page. */
export const pluginStartPageFor = (installation: PluginInstallation, viewId: string): PluginStartPage => ({
  installationId: installation.installationId,
  pluginId: installation.manifest.id,
  viewId,
});

/**
 * The installation a preference names. An older preference saved only the
 * plugin id; it resolves only while exactly one installed plugin has that id
 * (any state), so two sources never get confused.
 */
export function pluginStartPageInstallationId(
  preference: PluginStartPage | null,
  installations: ReadonlyArray<PluginInstallation>,
): string | null {
  if (preference === null) return null;
  if (preference.installationId !== null) return preference.installationId;
  const matches = installations.filter((entry) => entry.manifest.id === preference.pluginId);
  return matches.length === 1 ? matches[0]!.installationId : null;
}

export type PluginStartPageResolution =
  | { readonly kind: "pending" }
  | { readonly kind: "core" }
  | {
      readonly kind: "open";
      readonly installationId: string;
      readonly viewId: string;
      /** An older preference resolved; save this so it stops depending on the plugin id. */
      readonly upgraded: PluginStartPage | null;
    };

/** Waits only while the plugin list is loading; everything else falls back at once. */
export function resolvePluginStartPage(input: {
  readonly preference: PluginStartPage | null;
  readonly catalogue: "pending" | "ready" | "unavailable";
  /** Every installation in the catalogue, runnable or not. */
  readonly installations: ReadonlyArray<PluginInstallation>;
  /** Safe mode: no plugin page opens. */
  readonly executionDisabled: boolean;
}): PluginStartPageResolution {
  const { preference } = input;
  if (preference === null || input.catalogue === "unavailable" || input.executionDisabled) return { kind: "core" };
  if (input.catalogue === "pending") return { kind: "pending" };
  const installationId = pluginStartPageInstallationId(preference, input.installations);
  const installation = input.installations.find(
    (entry) =>
      entry.installationId === installationId && isPluginActive(entry) && entry.grants.capabilities.includes("ui"),
  );
  const view = installation?.manifest.views.find((entry) => entry.id === preference.viewId && entry.placement === "page");
  if (installation === undefined || view === undefined) return { kind: "core" };
  return {
    kind: "open",
    installationId: installation.installationId,
    viewId: view.id,
    upgraded: preference.installationId === null ? pluginStartPageFor(installation, view.id) : null,
  };
}
