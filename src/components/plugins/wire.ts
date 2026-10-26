/**
 * Reads what the plugin host actually sends into the UI's contract shapes.
 * The host writes `null` for absent optional fields, keeps unknown fields and
 * values from newer versions, and returns a placeholder for records it could
 * not read. Nothing here throws: a malformed entry loses the parts the UI
 * cannot use (an unknown view placement, a setting type it cannot edit), and
 * an entry without an installation id is dropped, so one bad record never
 * takes the plugin list, the sidebar or a transcript down with it.
 */
import type {
  PluginAction,
  PluginCatalogue,
  PluginClientIntent,
  PluginInstallation,
  PluginInstallationStatus,
  PluginInvokeResult,
  PluginManifest,
  PluginNativeTree,
  PluginPendingUpdate,
  PluginRepairWorkspace,
  PluginResultImage,
  PluginRevisionKind,
  PluginRuntimeObservation,
  PluginSetting,
  PluginSettingValue,
  PluginSidebarView,
  PluginSource,
  PluginThreadStatusItem,
  PluginView,
} from "./contract";
import { decodePluginIntent } from "./intents";

type Data = Record<string, unknown>;

const isRecord = (value: unknown): value is Data =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const text = (value: unknown): string | undefined => (typeof value === "string" ? value : undefined);
const texts = (value: unknown): string[] =>
  Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === "string") : [];
const records = (value: unknown): Data[] => (Array.isArray(value) ? value.filter(isRecord) : []);
/** `null` and absent both mean "not set". */
const optional = <K extends string, V>(key: K, value: V | undefined): { [P in K]?: V } =>
  (value === undefined ? {} : { [key]: value }) as { [P in K]?: V };

const VIEW_PLACEMENTS = new Set(["page", "side-panel", "conversation"]);
const SETTING_TYPES = new Set(["string", "boolean", "number", "secret"]);
const STATUSES = new Set<PluginInstallationStatus>([
  "needs-consent",
  "disabled",
  "idle",
  "running",
  "error",
  "quarantined",
  "incompatible",
]);
const REVISION_KINDS = new Set<PluginRevisionKind>(["upstream", "local-repair", "previous"]);

const settingValue = (value: unknown): PluginSettingValue | undefined =>
  typeof value === "string" || typeof value === "boolean" || (typeof value === "number" && Number.isFinite(value))
    ? value
    : undefined;

export function readPluginSource(value: unknown): PluginSource | null {
  if (!isRecord(value)) return null;
  if (value.kind === "local") {
    const directory = text(value.directory);
    return directory === undefined ? null : { kind: "local", directory };
  }
  if (value.kind === "git") {
    const url = text(value.url);
    if (url === undefined) return null;
    return {
      kind: "git",
      url,
      ...optional("ref", text(value.ref)),
      ...optional("commit", text(value.commit)),
      ...optional("subdir", text(value.subdir)),
    };
  }
  return null;
}

function readAction(value: Data): PluginAction | null {
  const id = text(value.id);
  const title = text(value.title);
  if (id === undefined || title === undefined) return null;
  return {
    id,
    title,
    ...optional("description", text(value.description)),
    placements: texts(value.placements),
    ...(value.agentCallable === true ? { agentCallable: true } : {}),
  };
}

/** A view at a placement this client does not know is never offered. */
function readView(value: Data): PluginView | null {
  const id = text(value.id);
  const title = text(value.title);
  const placement = text(value.placement);
  if (id === undefined || title === undefined || placement === undefined || !VIEW_PLACEMENTS.has(placement)) {
    return null;
  }
  return { id, title, entry: text(value.entry) ?? "", placement: placement as PluginView["placement"] };
}

/** A setting type this client cannot edit is left to the host; its stored value is untouched. */
function readSetting(value: Data): PluginSetting | null {
  const key = text(value.key);
  const label = text(value.label);
  const type = text(value.type);
  if (key === undefined || label === undefined || type === undefined || !SETTING_TYPES.has(type)) return null;
  return {
    key,
    label,
    type: type as PluginSetting["type"],
    ...optional("description", text(value.description)),
    ...optional("default", settingValue(value.default)),
  };
}

function readStatusItem(value: Data): PluginThreadStatusItem | null {
  const id = text(value.id);
  const title = text(value.title);
  return id === undefined || title === undefined ? null : { id, title, ...optional("viewId", text(value.viewId)) };
}

/** Rule values are kept as sent (minus nulls); the sidebar drops views whose rules it does not know. */
function readSidebarView(value: Data): PluginSidebarView | null {
  const id = text(value.id);
  const title = text(value.title);
  if (id === undefined || title === undefined) return null;
  const view: Data = { id, title, groupBy: value.groupBy };
  if (typeof value.description === "string") view.description = value.description;
  if (value.quietAfterDays !== undefined && value.quietAfterDays !== null) view.quietAfterDays = value.quietAfterDays;
  if (value.emptyProjects !== undefined && value.emptyProjects !== null) view.emptyProjects = value.emptyProjects;
  return view as unknown as PluginSidebarView;
}

function readNativeTree(value: Data): PluginNativeTree | null {
  const id = text(value.id);
  const title = text(value.title);
  return id === undefined || title === undefined ? null : { id, title, ...optional("description", text(value.description)) };
}

const each = <T>(value: unknown, read: (entry: Data) => T | null): T[] =>
  records(value).flatMap((entry) => {
    const result = read(entry);
    return result === null ? [] : [result];
  });

export function readPluginManifest(value: unknown, fallbackId: string): PluginManifest {
  const raw = isRecord(value) ? value : {};
  const id = text(raw.id) ?? fallbackId;
  const optionalCapabilities = texts(raw.optionalCapabilities);
  return {
    id,
    name: text(raw.name) ?? id,
    ...optional("description", text(raw.description)),
    version: text(raw.version) ?? "0.0.0",
    apiVersion: typeof raw.apiVersion === "number" ? raw.apiVersion : 0,
    entry: text(raw.entry) ?? "",
    capabilities: [...new Set([...texts(raw.capabilities), ...texts(raw.requiredCapabilities)])],
    ...(optionalCapabilities.length > 0 ? { optionalCapabilities } : {}),
    actions: each(raw.actions, readAction),
    views: each(raw.views, readView),
    settings: each(raw.settings, readSetting),
    threadStatus: each(raw.threadStatus ?? raw.status, readStatusItem),
    sidebarViews: each(raw.sidebarViews, readSidebarView),
    nativeTrees: each(raw.nativeTrees, readNativeTree),
  };
}

function readObservation(value: unknown): PluginRuntimeObservation | undefined {
  if (!isRecord(value)) return undefined;
  const hostVersion = text(value.hostVersion);
  const nodeVersion = text(value.nodeVersion);
  const at = text(value.at);
  return hostVersion === undefined || nodeVersion === undefined || at === undefined
    ? undefined
    : { hostVersion, nodeVersion, at };
}

function readHealth(value: unknown): PluginInstallation["health"] {
  if (!isRecord(value)) return undefined;
  const lastHealthy = readObservation(value.lastHealthy);
  const faultObservation = readObservation(value.fault);
  const fault =
    faultObservation !== undefined && isRecord(value.fault)
      ? {
          ...faultObservation,
          reason: text(value.fault.reason) ?? "operation_failed",
          message: text(value.fault.message) ?? "The plugin failed.",
          ...optional("handler", text(value.fault.handler)),
        }
      : undefined;
  return lastHealthy === undefined && fault === undefined
    ? undefined
    : { ...optional("lastHealthy", lastHealthy), ...optional("fault", fault) };
}

function readRepair(value: unknown): PluginRepairWorkspace | undefined {
  if (!isRecord(value)) return undefined;
  const fields = ["workspaceId", "directory", "baseDigest", "baseGeneration", "draft"] as const;
  const read = Object.fromEntries(fields.map((field) => [field, text(value[field])]));
  return fields.every((field) => read[field] !== undefined) ? (read as unknown as PluginRepairWorkspace) : undefined;
}

function readRecovery(value: unknown): PluginInstallation["recovery"] {
  if (!isRecord(value)) return undefined;
  const previous = isRecord(value.previous) ? value.previous : null;
  const previousVersion = previous ? text(previous.version) : undefined;
  const previousDigest = previous ? text(previous.digest) : undefined;
  const previousSource = previous ? readPluginSource(previous.source) : null;
  const upstreamSource = readPluginSource(value.upstreamSource);
  return {
    activeSource: value.activeSource === "local-repair" ? "local-repair" : "upstream",
    ...(previousVersion !== undefined && previousDigest !== undefined && previousSource !== null
      ? { previous: { version: previousVersion, digest: previousDigest, source: previousSource } }
      : {}),
    ...optional("repair", readRepair(value.repair)),
    ...(upstreamSource === null ? {} : { upstreamSource }),
  };
}

function readPendingUpdate(value: unknown, fallbackId: string): PluginPendingUpdate | undefined {
  if (!isRecord(value)) return undefined;
  const source = readPluginSource(value.source);
  const digest = text(value.digest);
  if (source === null || digest === undefined || !isRecord(value.manifest)) return undefined;
  const kind = text(value.kind);
  return {
    ...(kind !== undefined && REVISION_KINDS.has(kind as PluginRevisionKind) ? { kind: kind as PluginRevisionKind } : {}),
    manifest: readPluginManifest(value.manifest, fallbackId),
    source,
    digest,
    generation: text(value.generation) ?? `pending-${digest}`,
    updatedAt: text(value.updatedAt) ?? "",
  };
}

function readSettings(value: unknown): { settings: Record<string, PluginSettingValue>; storedSecrets: string[] } {
  const settings: Record<string, PluginSettingValue> = {};
  const storedSecrets: string[] = [];
  if (!isRecord(value)) return { settings, storedSecrets };
  for (const [key, entry] of Object.entries(value)) {
    if (isRecord(entry)) {
      if (entry.hasSecret === true) storedSecrets.push(key);
      continue;
    }
    const plain = settingValue(entry);
    if (plain !== undefined) settings[key] = plain;
  }
  return { settings, storedSecrets };
}

/** An installation as the UI uses it, or null when it has no usable identity. */
export function readPluginInstallation(value: unknown): PluginInstallation | null {
  if (!isRecord(value)) return null;
  const installationId = text(value.installationId);
  if (installationId === undefined || installationId === "") return null;
  const manifest = readPluginManifest(value.manifest, installationId);
  const status = text(value.status);
  const compatibility = isRecord(value.compatibility) ? value.compatibility : null;
  const attention = isRecord(value.attention) ? value.attention : null;
  const grants = isRecord(value.grants) ? value.grants : {};
  const { settings, storedSecrets } = readSettings(value.settings);
  const health = readHealth(value.health);
  const recovery = readRecovery(value.recovery);
  const pendingUpdate = readPendingUpdate(value.pendingUpdate, installationId);
  const observerError = value.observerError;
  return {
    installationId,
    generation: text(value.generation) ?? "unavailable",
    ...optional("dataOwner", text(value.dataOwner)),
    ...optional("pendingUpdate", pendingUpdate),
    ...optional("recordIssue", text(value.recordIssue)),
    ...(compatibility === null
      ? {}
      : {
          compatibility: {
            available: compatibility.available === true,
            missingCapabilities: texts(compatibility.missingCapabilities),
            unsupportedRequirements: texts(compatibility.unsupportedRequirements),
          },
        }),
    ...optional("health", health),
    ...(attention !== null && typeof attention.kind === "string" && typeof attention.message === "string"
      ? { attention: { kind: attention.kind, message: attention.message, since: text(attention.since) ?? "" } }
      : {}),
    ...optional("recovery", recovery),
    manifest,
    source: readPluginSource(value.source) ?? { kind: "local", directory: "" },
    digest: text(value.digest) ?? "",
    enabled: value.enabled === true,
    consentedDigest: text(value.consentedDigest) ?? null,
    // A state from a newer host runs nothing here until this one understands it.
    status: status !== undefined && STATUSES.has(status as PluginInstallationStatus)
      ? (status as PluginInstallationStatus)
      : "incompatible",
    error: text(value.error) ?? null,
    ...(typeof observerError === "string" ? { observerError } : {}),
    settings,
    ...(storedSecrets.length > 0 ? { storedSecrets } : {}),
    grants: { capabilities: texts(grants.capabilities), projectIds: texts(grants.projectIds) },
    updatedAt: text(value.updatedAt) ?? "",
  };
}

/** The catalogue, or null when the answer is not one (an older or broken host). */
export function readPluginCatalogue(value: unknown): PluginCatalogue | null {
  if (!isRecord(value) || !Array.isArray(value.installations)) return null;
  const unavailable = isRecord(value.unavailable) ? text(value.unavailable.message) : undefined;
  return {
    ...(value.executionDisabled === true ? { executionDisabled: true } : {}),
    ...(unavailable === undefined ? {} : { unavailable: { message: unavailable } }),
    ...(value.recoverySupported === true ? { recoverySupported: true } : {}),
    ...(value.changesSupported === true ? { changesSupported: true } : {}),
    ...(value.conversationCardsSupported === false ? { conversationCardsSupported: false } : {}),
    installations: value.installations.flatMap((entry) => {
      const installation = readPluginInstallation(entry);
      return installation === null ? [] : [installation];
    }),
  };
}

function readImage(value: Data): PluginResultImage | null {
  const data = text(value.data);
  return (value.mimeType === "image/png" || value.mimeType === "image/jpeg") && data !== undefined
    ? { mimeType: value.mimeType, data }
    : null;
}

/**
 * An invoke answer. Intents this client cannot read are counted, never
 * applied, so a newer plugin degrades to a notice. A card is left out: a
 * person's invocation shows no card, and transcript cards are read from the
 * recorded tool result (`cards.ts`).
 */
export function readPluginInvokeResult(value: unknown): PluginInvokeResult {
  if (!isRecord(value)) return {};
  const intents: PluginClientIntent[] = [];
  let unsupported =
    typeof value.unsupportedIntentCount === "number" &&
    Number.isSafeInteger(value.unsupportedIntentCount) &&
    value.unsupportedIntentCount > 0
      ? value.unsupportedIntentCount
      : 0;
  for (const entry of Array.isArray(value.intents) ? value.intents : []) {
    const intent = decodePluginIntent(entry);
    if (intent === null) unsupported += 1;
    else intents.push(intent);
  }
  const images = each(value.images, readImage);
  return {
    ...optional("message", text(value.message)),
    ...(value.data === undefined ? {} : { data: value.data }),
    ...(intents.length > 0 ? { intents } : {}),
    ...(unsupported > 0 ? { unsupportedIntentCount: unsupported } : {}),
    ...(images.length > 0 ? { images } : {}),
  };
}
