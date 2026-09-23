/** Trusted bridge data only. Never import the SDK runtime into the app renderer. */
export interface AddonViewDeclaration {
  id: string;
  title: string;
  icon: string;
}
export type AddonSetting =
  | { id: string; label: string; type: "boolean"; default: boolean }
  | { id: string; label: string; type: "string"; default: string }
  | {
      id: string;
      label: string;
      type: "integer";
      default: number;
      min: number;
      max: number;
    }
  | {
      id: string;
      label: string;
      type: "enum";
      default: string;
      values: string[];
    };
export interface AddonManifest {
  id: string;
  name: string;
  description: string;
  version: string;
  api: string;
  author: { name: string; url: string };
  repository: string;
  license: string;
  platforms: string[];
  permissions: string[];
  http: { origin: string; methods: string[]; credential: string | null }[];
  credentials: { id: string; label: string; origin: string; type: "bearer" }[];
  contributes: {
    commands: { id: string; title: string; requiresWorkspace: boolean }[];
    panels: AddonViewDeclaration[];
    composerActions: AddonViewDeclaration[];
    composerViews: AddonViewDeclaration[];
  };
  settings: AddonSetting[];
}
export type AddonSource =
  | { kind: "local"; identity: string }
  | { kind: "catalog"; publisher: string; repository: string };
/** Reviewed catalog identity; `tier` and `listed` come from the cached catalog. */
export interface AddonListing {
  publisher: string;
  repository: string;
  tier: "official" | "community" | null;
  listed: boolean;
}
export interface AddonCompatibility {
  api: string;
  hostApi: string;
  platforms: string[];
  platform: string | null;
  compatible: boolean;
  reason: string | null;
}
export interface AddonInstallation {
  installationId: string;
  manifest: AddonManifest;
  source: AddonSource;
  digest: string;
  dataGeneration: string;
  desiredEnabled: boolean;
  status:
    | "installed-disabled"
    | "activating"
    | "enabled-idle"
    | "enabled-running"
    | "updating"
    | "failed-disabled"
    | "incompatible-disabled"
    | "blocked-disabled"
    | "removing";
  failure: string | null;
  previous: { manifest: AddonManifest } | null;
  /** Newer compatible, unblocked catalog version from the cached snapshot. */
  updateAvailable?: string | null;
  /** Present for catalog-source installations only. */
  catalog?: AddonListing | null;
  compatibility?: AddonCompatibility;
}
export interface AddonInventory {
  paused: boolean;
  installed: AddonInstallation[];
  error: string | null;
  warnings?: string[];
  developerMode?: boolean;
  developmentPackage?: string | null;
  /** Set when the registry could not be opened; `addon_registry_reset` recovers it. */
  registryError?: { path: string; cause: string } | null;
  /** Plugin IDs whose activation an unclean exit interrupted; cleared by Resume. */
  interruptedActivations?: string[];
}
export interface AddonError {
  message: string;
  data: { code: string };
  code?: number;
}
export interface AddonNode {
  id: string;
  type: number;
  element?: string;
  data?: string;
  properties: Record<string, unknown>;
  attributes: Record<string, unknown>;
  eventListeners: Record<string, { callbackId: string }>;
  children: AddonNode[];
}
export type AddonEvent =
  | { type: "development-review"; review: AddonReview }
  | { type: "development-error"; message: string }
  | { type: "inventory" }
  | {
      type: "tree";
      pluginId: string;
      generation: string;
      viewId: string;
      revision: number;
      tree: { children: AddonNode[] };
    }
  | {
      type: "effect";
      pluginId: string;
      generation: string;
      requestId: string;
      operation: string;
      params: Record<string, unknown>;
    }
  | { type: "stopped"; pluginId: string; generation: string; message: string };
export type AddonTreeEvent = Extract<AddonEvent, { type: "tree" }>;
export interface AddonMount {
  viewId: string;
  generation: string;
}
export interface AddonAccess {
  permissions: string[];
  http: { origin: string; methods: string[] }[];
  credentials: { id: string; label: string; origin: string }[];
}
export interface AddonReview {
  token: string;
  manifest: AddonManifest;
  digest: string;
  source: AddonSource;
  replacesSource: boolean;
  /** True only when `added` is non-empty. */
  expandsAccess: boolean;
  compressedBytes: number;
  development?: boolean;
  retainedData?: { version: string } | null;
  /** The currently installed release, when this review updates or replaces it. */
  installed?: {
    version: string;
    digest: string;
    source: AddonSource;
    desiredEnabled: boolean;
    capabilities: {
      permissions: string[];
      http: AddonManifest["http"];
      credentials: AddonManifest["credentials"];
    };
  } | null;
  /** Access the candidate adds, or all of it for a new installation. */
  added?: AddonAccess;
  /** Access the installed release has and the candidate drops. */
  removed?: AddonAccess;
  catalog?: AddonListing | null;
}
export interface AddonUpdateCheck {
  upToDate: boolean;
  installedVersion: string;
  availableVersion: string | null;
  review: AddonReview | null;
}
export function addonMessage(error: unknown): string {
  return typeof error === "object" &&
    error !== null &&
    "message" in error &&
    typeof error.message === "string"
    ? error.message
    : typeof error === "string"
      ? error
      : "The add-on operation failed";
}
export function addonError(code: string, message: string): AddonError {
  return { message, data: { code } };
}
export function addonEnabled(installation: AddonInstallation): boolean {
  return (
    installation.desiredEnabled &&
    ["enabled-idle", "enabled-running", "activating"].includes(
      installation.status,
    )
  );
}
