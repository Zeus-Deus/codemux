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
}
/**
 * Host-owned state of one declared credential; never the secret itself.
 * "not-configured" means requests to its origin are sent unauthenticated.
 * "cleanup-pending" means a cleared saved value still awaits OS-store removal;
 * it is already unusable and `addon_retry_cleanup` retries the removal.
 */
export type AddonCredentialState =
  | "not-configured"
  | "saved"
  | "session-only"
  | "cleanup-pending";
export interface AddonInventory {
  paused: boolean;
  installed: AddonInstallation[];
  error: string | null;
  warnings?: string[];
  developerMode?: boolean;
  developmentPackage?: string | null;
  /** Plugin ID -> declared credential ID -> state. */
  credentialStates?: Record<string, Record<string, AddonCredentialState>>;
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
export interface AddonReview {
  token: string;
  manifest: AddonManifest;
  digest: string;
  source: AddonSource;
  replacesSource: boolean;
  expandsAccess: boolean;
  compressedBytes: number;
  development?: boolean;
  retainedData?: { version: string } | null;
}
/** `addon_diagnostics` log entry. Plugin log text is never kept. */
export interface AddonLogEntry {
  /** Milliseconds since the Unix epoch. */
  at: number;
  level: "log" | "info" | "warn" | "error" | "debug";
  /** UTF-8 size of the entry after host truncation (at most 1,024 characters). */
  bytes: number;
}
/** Current installation's log activity this session, kept across stops and crashes. */
export interface AddonDiagnostics {
  /** Entries received this session, including ones the ring evicted. */
  received: number;
  /** Oldest first; at most 1,024 entries (a 64 KiB ring). */
  logs: AddonLogEntry[];
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
