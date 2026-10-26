/**
 * Wire types for isolated platform plugins (`codemux-plugin.json`, API v1), as
 * the UI reads them. The vocabulary follows the v1 platform contract: an
 * installation is one plugin installed from a source, pinned to an exact
 * digest, with a `generation` that changes whenever the code that would run
 * changes. These are the shapes after `wire.ts` normalizes what the host
 * sent (it writes `null` for absent optional fields and may send fields or
 * values from a newer version); nothing reads the raw response directly.
 */

export const PLUGIN_API_VERSION = 1;

export const PLUGIN_CAPABILITIES = [
  "storage",
  "settings",
  "threads.read",
  "threads.send",
  "threads.create",
  "events.run-finalized",
  "ui",
  "ui.sidebar-views",
  "ui.native-trees",
  "ui.conversation-cards",
  "ui.changes",
  "worktrees.create",
  "workflow.query",
] as const;
export type PluginCapability = (typeof PLUGIN_CAPABILITIES)[number];

export type PluginActionPlacement = "palette" | "thread-menu" | "message-selection";

export interface PluginAction {
  id: string;
  title: string;
  description?: string;
  /** Unknown placements from a newer host are kept but never matched. */
  placements: ReadonlyArray<PluginActionPlacement | (string & {})>;
  agentCallable?: boolean;
}

export type PluginViewPlacement = "page" | "side-panel" | "conversation";

export interface PluginView {
  id: string;
  title: string;
  entry: string;
  placement: PluginViewPlacement;
}

export interface PluginThreadStatusItem {
  id: string;
  title: string;
  viewId?: string;
}

export type PluginThreadStatusTone = "neutral" | "info" | "success" | "warning" | "danger";

export interface PluginThreadStatus {
  label: string;
  tone: PluginThreadStatusTone;
  detail?: string;
}

/**
 * An alternative arrangement of the sidebar's Projects view. The plugin only
 * declares rules; the host derives the view from rows it already lists, so no
 * chat data reaches the plugin. Needs `ui.sidebar-views`.
 */
export interface PluginSidebarView {
  id: string;
  title: string;
  description?: string;
  groupBy: "project";
  /** Projects with nothing live and no activity for this many days fold away. Absent: never. */
  quietAfterDays?: number;
  /** Whether projects without any chats get a heading. Defaults to `hide`. */
  emptyProjects?: "show" | "hide";
}

export interface PluginNativeTree {
  id: string;
  title: string;
  description?: string;
}

export const MAX_PLUGIN_TREE_NODES = 2_000;

interface TreeNodeFields {
  id: string;
  parentId?: string;
  collapsedByDefault?: boolean;
}

export type PluginTreeNode =
  | (TreeNodeFields & { kind: "group"; title: string })
  | (TreeNodeFields & { kind: "project"; projectId: string })
  | (TreeNodeFields & { kind: "thread"; threadId: string });

export interface PluginTreeSnapshot {
  nodes: ReadonlyArray<PluginTreeNode>;
}

export type PluginSettingValue = string | number | boolean;

export interface PluginSetting {
  key: string;
  label: string;
  description?: string;
  type: "string" | "boolean" | "number" | "secret";
  default?: PluginSettingValue;
}

export interface PluginManifest {
  id: string;
  name: string;
  description?: string;
  version: string;
  /** 1 for this contract; anything else is inspectable but never runs. */
  apiVersion: number;
  entry: string;
  /**
   * Required capabilities (the host's `capabilities` and `requiredCapabilities`
   * together). Unknown requirements are kept so the UI can say why a plugin
   * cannot run.
   */
  capabilities: ReadonlyArray<string>;
  optionalCapabilities?: ReadonlyArray<string>;
  actions: ReadonlyArray<PluginAction>;
  views: ReadonlyArray<PluginView>;
  settings: ReadonlyArray<PluginSetting>;
  threadStatus?: ReadonlyArray<PluginThreadStatusItem>;
  sidebarViews?: ReadonlyArray<PluginSidebarView>;
  nativeTrees?: ReadonlyArray<PluginNativeTree>;
}

export type PluginSource =
  | { kind: "local"; directory: string }
  | { kind: "git"; url: string; ref?: string; commit?: string; subdir?: string };

export interface PluginGrants {
  capabilities: ReadonlyArray<string>;
  projectIds: ReadonlyArray<string>;
}

export interface PluginEventBacklogPolicy {
  policy: "skip" | "deliver";
  throughSequence: number;
}

export interface PluginEventBacklog {
  count: number;
  afterSequence: number;
  throughSequence: number;
  requiresChoice: boolean;
}

export interface PluginRepairWorkspace {
  workspaceId: string;
  directory: string;
  baseDigest: string;
  baseGeneration: string;
  draft: string;
}

export type PluginRevisionKind = "upstream" | "local-repair" | "previous";

export interface PluginRuntimeObservation {
  hostVersion: string;
  /** Runtime that ran the plugin (the bundled worker runtime's version). */
  nodeVersion: string;
  at: string;
}

export interface PluginPendingUpdate {
  kind?: PluginRevisionKind;
  manifest: PluginManifest;
  source: PluginSource;
  digest: string;
  generation: string;
  updatedAt: string;
}

export type PluginInstallationStatus =
  | "needs-consent"
  | "disabled"
  | "idle"
  | "running"
  | "error"
  | "quarantined"
  | "incompatible";

export interface PluginInstallation {
  installationId: string;
  generation: string;
  /** Matches retained card data after a reinstall; the host still authorizes every read. */
  dataOwner?: string;
  pendingUpdate?: PluginPendingUpdate;
  recordIssue?: string;
  compatibility?: {
    available: boolean;
    missingCapabilities: ReadonlyArray<string>;
    /** `requires` keys this host cannot satisfy (a newer API or host version). */
    unsupportedRequirements?: ReadonlyArray<string>;
  };
  health?: {
    lastHealthy?: PluginRuntimeObservation;
    fault?: PluginRuntimeObservation & { reason: string; message: string; handler?: string };
  };
  attention?: { kind: string; message: string; since: string };
  recovery?: {
    activeSource: "upstream" | "local-repair";
    previous?: { version: string; digest: string; source: PluginSource };
    repair?: PluginRepairWorkspace;
    upstreamSource?: PluginSource;
  };
  manifest: PluginManifest;
  source: PluginSource;
  digest: string;
  /** The user's choice. It survives incompatibility, quarantine and app updates. */
  enabled: boolean;
  consentedDigest: string | null;
  status: PluginInstallationStatus;
  error: string | null;
  observerError?: string | null;
  /** Plain values only; secrets are never echoed. */
  settings: Readonly<Record<string, PluginSettingValue>>;
  /** Secret settings the host holds a value for. */
  storedSecrets?: ReadonlyArray<string>;
  grants: PluginGrants;
  updatedAt: string;
}

export interface PluginCatalogue {
  /** Safe mode: no plugin code runs; plugins can still be reviewed, disabled or removed. */
  executionDisabled?: boolean;
  /** The plugin store could not be opened. Core work is unaffected. */
  unavailable?: { message: string };
  /** The host supports repair, previous versions and missed-event choices. */
  recoverySupported?: boolean;
  /** The host sends `plugins_platform_changed` signals. */
  changesSupported?: boolean;
  /** The host serves conversation card views. */
  conversationCardsSupported?: boolean;
  installations: ReadonlyArray<PluginInstallation>;
}

export interface PluginActionContext {
  projectId?: string;
  threadId?: string;
  messageId?: string;
  selection?: string;
}

export interface PluginConversationCard {
  viewId: string;
  summary: string;
  data?: unknown;
}

/** Host-stamped provenance. A retained data owner may mount a later generation; identity grants nothing. */
export interface PluginCardReference extends PluginConversationCard {
  installationId: string;
  generation: string;
  dataOwner: string;
}

export const MAX_PLUGIN_CARD_DATA_BYTES = 2_048;
export const MAX_PLUGIN_CARD_BYTES = 4_096;

export type PluginClientIntent =
  | { type: "composer.insert"; text: string; quote?: boolean }
  | { type: "preview.open"; url: string }
  | { type: "thread.reveal"; threadId: string; messageId?: string }
  | { type: "view.open"; viewId: string };

export interface PluginResultImage {
  mimeType: "image/png" | "image/jpeg";
  data: string;
}

export interface PluginInvokeInput {
  installationId: string;
  generation: string;
  handler: string;
  input?: unknown;
  context?: PluginActionContext;
  operationId: string;
}

export interface PluginInvokeResult {
  card?: PluginCardReference;
  message?: string;
  data?: unknown;
  intents?: ReadonlyArray<PluginClientIntent>;
  unsupportedIntentCount?: number;
  images?: ReadonlyArray<PluginResultImage>;
}

export interface PluginChanged {
  installationId: string;
  generation: string;
  topic: string;
}

/** What a failed request carries. Unknown reasons from a newer host still show their message. */
export interface PluginFailure {
  reason: string;
  message: string;
  code?: string;
}
