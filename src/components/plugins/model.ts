/**
 * Pure presentation rules for platform plugins: what may run, what a review
 * approves, how a record's state reads, and which recovery steps apply. No
 * React, no IPC: the UI and tests share these.
 */
import {
  PLUGIN_CAPABILITIES,
  type PluginAction,
  type PluginActionPlacement,
  type PluginCapability,
  type PluginCatalogue,
  type PluginEventBacklog,
  type PluginEventBacklogPolicy,
  type PluginFailure,
  type PluginGrants,
  type PluginInstallation,
  type PluginManifest,
  type PluginSetting,
  type PluginSettingValue,
  type PluginSource,
  type PluginThreadStatusItem,
  type PluginView,
} from "./contract";

// ── activity ──────────────────────────────────────────────────────────

/**
 * Ready to run code: enabled, consent matches the installed bytes, supported
 * by this host, and not stopped by failure.
 */
export function isPluginActive(installation: PluginInstallation): boolean {
  return (
    installation.enabled &&
    installation.consentedDigest === installation.digest &&
    installation.compatibility?.available !== false &&
    (installation.status === "idle" || installation.status === "running")
  );
}

/** None while plugin code is off (safe mode): the host refuses them, so nothing offers them. */
export const pluginRunnableInstallations = (
  catalogue: Pick<PluginCatalogue, "installations" | "executionDisabled"> | null,
): ReadonlyArray<PluginInstallation> =>
  catalogue === null || catalogue.executionDisabled === true ? [] : catalogue.installations;

export interface PluginActionEntry {
  readonly installation: PluginInstallation;
  readonly action: PluginAction;
}

export interface PluginViewEntry {
  readonly installation: PluginInstallation;
  readonly view: PluginView;
}

export interface PluginThreadStatusEntry {
  readonly installation: PluginInstallation;
  readonly item: PluginThreadStatusItem;
}

const byTitle =
  <T extends { readonly installation: PluginInstallation }>(title: (entry: T) => string) =>
  (left: T, right: T) =>
    left.installation.manifest.name.localeCompare(right.installation.manifest.name) ||
    title(left).localeCompare(title(right));

const grants = (installation: PluginInstallation, capability: PluginCapability) =>
  installation.grants.capabilities.includes(capability);

export function pluginActionEntries(
  installations: ReadonlyArray<PluginInstallation>,
  placement: PluginActionPlacement,
): ReadonlyArray<PluginActionEntry> {
  return installations
    .filter(isPluginActive)
    .flatMap((installation) =>
      installation.manifest.actions
        .filter((action) => action.placements.includes(placement))
        .map((action) => ({ installation, action })),
    )
    .sort(byTitle((entry) => entry.action.title));
}

/** Views a person can open directly. Conversation views only open from their card. */
export function pluginViewEntries(
  installations: ReadonlyArray<PluginInstallation>,
): ReadonlyArray<PluginViewEntry> {
  return installations
    .filter((installation) => isPluginActive(installation) && grants(installation, "ui"))
    .flatMap((installation) =>
      installation.manifest.views
        .filter((view) => view.placement !== "conversation")
        .map((view) => ({ installation, view })),
    )
    .sort(byTitle((entry) => entry.view.title));
}

/** Statuses render host-side, but the host answers them only with `ui` granted. */
export function pluginThreadStatusEntries(
  installations: ReadonlyArray<PluginInstallation>,
): ReadonlyArray<PluginThreadStatusEntry> {
  return installations
    .filter((installation) => isPluginActive(installation) && grants(installation, "ui"))
    .flatMap((installation) =>
      (installation.manifest.threadStatus ?? []).map((item) => ({ installation, item })),
    )
    .sort(byTitle((entry) => entry.item.title));
}

export function findPluginView(
  installations: ReadonlyArray<PluginInstallation>,
  installationId: string,
  viewId: string,
): PluginViewEntry | null {
  const installation = installations.find((entry) => entry.installationId === installationId);
  const view = installation?.manifest.views.find((entry) => entry.id === viewId);
  return installation && view ? { installation, view } : null;
}

export const pluginActionHandler = (actionId: string) => `action:${actionId}`;
export const pluginViewHandler = (viewId: string, name: string) => `view:${viewId}:${name}`;
export const pluginStatusHandler = (statusId: string) => `status:${statusId}`;

const VIEW_HANDLER_NAME = /^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/;
export const isPluginViewHandlerName = (name: unknown): name is string =>
  typeof name === "string" && VIEW_HANDLER_NAME.test(name);

const randomHex = () =>
  Array.from(crypto.getRandomValues(new Uint8Array(16)), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");

/**
 * One id per user invocation; retries of that invocation reuse it so the host
 * can dedupe. `crypto.randomUUID` is missing over plain-HTTP LAN origins, so
 * this uses `getRandomValues`, which is not.
 */
export function newPluginOperationId(random: () => string = randomHex): string {
  return `op-${random()
    .replace(/[^a-zA-Z0-9]/g, "")
    .slice(0, 64)}`;
}

// ── capabilities ──────────────────────────────────────────────────────

export const pluginRequestedCapabilities = (
  manifest: Pick<PluginManifest, "capabilities" | "optionalCapabilities">,
): string[] => [...new Set([...manifest.capabilities, ...(manifest.optionalCapabilities ?? [])])];

interface PluginCapabilityText {
  /** The plugin runs without it, so an unknown optional capability does not block it. */
  readonly optional: boolean;
  readonly title: string;
  readonly detail: string;
  /** Needs projects selected before the grant does anything. */
  readonly projectScoped: boolean;
}

export type PluginCapabilityDescription =
  | (PluginCapabilityText & { readonly known: true; readonly capability: PluginCapability })
  | (PluginCapabilityText & { readonly known: false; readonly capability: string });

const CAPABILITY_TEXT: Record<
  PluginCapability,
  { title: string; detail: string; projectScoped: boolean }
> = {
  storage: { title: "Private storage", detail: "Keep its own data on this computer.", projectScoped: false },
  settings: {
    title: "Read its settings",
    detail: "Read the values you enter for it, including secrets.",
    projectScoped: false,
  },
  "threads.read": {
    title: "Read chats",
    detail: "Read messages in chats of the projects you choose.",
    projectScoped: true,
  },
  "threads.send": {
    title: "Send messages",
    detail: "Send messages to existing chats of the projects you choose.",
    projectScoped: true,
  },
  "threads.create": {
    title: "Start chats",
    detail: "Start new chats in the projects you choose.",
    projectScoped: true,
  },
  "events.run-finalized": {
    title: "Run in the background",
    detail: "Be told when a run finishes, even with no window open.",
    projectScoped: true,
  },
  ui: { title: "Show views", detail: "Render its own pages in a sandboxed frame.", projectScoped: false },
  "ui.sidebar-views": {
    title: "Arrange the sidebar",
    detail: "Offer another layout for your chat list. Codemux draws it; the plugin never reads chats.",
    projectScoped: false,
  },
  "ui.native-trees": {
    title: "Arrange chats in the sidebar",
    detail: "Group and nest the chats it can read in its own sidebar layout. Codemux draws the rows.",
    projectScoped: false,
  },
  "ui.conversation-cards": {
    title: "Show cards in chat",
    detail: "Show its own interactive cards in a conversation when an agent uses it.",
    projectScoped: false,
  },
  "ui.changes": {
    title: "Refresh its views",
    detail: "Tell its open views and sidebar layout when its data changed.",
    projectScoped: false,
  },
  "worktrees.create": {
    title: "Create worktrees",
    detail: "Start new chats in a new git worktree of the projects you choose.",
    projectScoped: true,
  },
  "workflow.query": {
    title: "List projects, chats and models",
    detail:
      "See titles, states and settings of chats in the projects you choose, without their messages, and the models they can use.",
    projectScoped: true,
  },
};

export const isKnownCapability = (capability: string): capability is PluginCapability =>
  (PLUGIN_CAPABILITIES as ReadonlyArray<string>).includes(capability);

export function describePluginCapabilities(
  manifest: Pick<PluginManifest, "capabilities" | "optionalCapabilities">,
): ReadonlyArray<PluginCapabilityDescription> {
  const required = new Set(manifest.capabilities);
  return pluginRequestedCapabilities(manifest).map((capability) => {
    const optional = !required.has(capability);
    return isKnownCapability(capability)
      ? { capability, known: true as const, optional, ...CAPABILITY_TEXT[capability] }
      : {
          capability,
          known: false as const,
          optional,
          title: capability,
          detail: optional
            ? "Not supported by this version of Codemux. The plugin runs without it."
            : "Not supported by this version of Codemux. The plugin cannot be enabled.",
          projectScoped: false,
        };
  });
}

/** Unknown required capabilities only; unknown optional ones degrade instead of blocking. */
export const pluginHasUnsupportedRequirement = (manifest: PluginManifest) =>
  describePluginCapabilities(manifest).some((entry) => !entry.known && !entry.optional);

export const pluginNeedsProjects = (manifest: PluginManifest) =>
  describePluginCapabilities(manifest).some((entry) => entry.known && entry.projectScoped);

// ── review ────────────────────────────────────────────────────────────

/** Where a staged revision came from. */
export type PluginReviewRevision = "install" | "update" | "local-repair" | "previous";

export interface PluginSettingChange {
  readonly key: string;
  readonly label: string;
  readonly change: "added" | "removed" | "type-changed";
}

/**
 * What a review approves: a staged update when there is one (the installed
 * version stays selected until it is approved), otherwise the installed files.
 * `digest` is the exact fingerprint the approval names.
 */
export interface PluginReviewSubject {
  readonly kind: "install" | "update";
  readonly revision: PluginReviewRevision;
  readonly manifest: PluginManifest;
  readonly source: PluginSource;
  readonly digest: string;
  /** Requested by this version but not the running one; empty for a first review. */
  readonly newCapabilities: ReadonlyArray<string>;
  /** Requested by the running version but no longer by this one. */
  readonly removedCapabilities: ReadonlyArray<string>;
  readonly settingChanges: ReadonlyArray<PluginSettingChange>;
}

function pluginSettingChanges(
  running: PluginManifest,
  staged: PluginManifest,
): ReadonlyArray<PluginSettingChange> {
  const before = new Map(running.settings.map((setting) => [setting.key, setting]));
  const after = new Map(staged.settings.map((setting) => [setting.key, setting]));
  const changes: PluginSettingChange[] = [];
  for (const setting of staged.settings) {
    const previous = before.get(setting.key);
    if (previous === undefined) changes.push({ key: setting.key, label: setting.label, change: "added" });
    else if (previous.type !== setting.type) {
      changes.push({ key: setting.key, label: setting.label, change: "type-changed" });
    }
  }
  for (const setting of running.settings) {
    if (!after.has(setting.key)) changes.push({ key: setting.key, label: setting.label, change: "removed" });
  }
  return changes;
}

export function pluginReviewSubject(installation: PluginInstallation): PluginReviewSubject {
  const pending = installation.pendingUpdate;
  if (pending === undefined) {
    return {
      kind: "install",
      revision: "install",
      manifest: installation.manifest,
      source: installation.source,
      digest: installation.digest,
      newCapabilities: [],
      removedCapabilities: [],
      settingChanges: [],
    };
  }
  const running = pluginRequestedCapabilities(installation.manifest);
  const staged = pluginRequestedCapabilities(pending.manifest);
  return {
    kind: "update",
    revision: pending.kind === "local-repair" || pending.kind === "previous" ? pending.kind : "update",
    manifest: pending.manifest,
    source: pending.source,
    digest: pending.digest,
    newCapabilities: staged.filter((capability) => !running.includes(capability)),
    removedCapabilities: running.filter((capability) => !staged.includes(capability)),
    settingChanges: pluginSettingChanges(installation.manifest, pending.manifest),
  };
}

/**
 * The grants a review starts with. A first review offers everything requested
 * and known; a later one keeps what the user granted before, so re-approving
 * never widens access without a click.
 */
export function initialPluginGrants(
  installation: PluginInstallation,
  manifest: PluginManifest = pluginReviewSubject(installation).manifest,
): PluginGrants {
  const requested = pluginRequestedCapabilities(manifest).filter(isKnownCapability);
  return {
    capabilities:
      installation.consentedDigest === null
        ? requested
        : requested.filter((capability) => installation.grants.capabilities.includes(capability)),
    projectIds: installation.grants.projectIds,
  };
}

// ── status ────────────────────────────────────────────────────────────

export type PluginStatusTone = "success" | "warning" | "error" | "muted";

export interface PluginStatusPresentation {
  readonly label: string;
  readonly tone: PluginStatusTone;
  /** What the row's primary control should offer. */
  readonly primary: "review" | "resume" | "none";
}

/**
 * A record stored by a newer Codemux, or one this one could not read. The host
 * keeps it byte for byte and refuses changes, so the UI offers no management.
 */
export const isPluginRecordReadOnly = (installation: PluginInstallation) =>
  installation.recordIssue !== undefined || installation.attention?.kind === "read-only";

/** Copy for a read-only record. It never suggests reinstalling: that would orphan its data. */
export function pluginRecordIssueMessage(installation: PluginInstallation): string | null {
  if (!isPluginRecordReadOnly(installation)) return null;
  return installation.recordIssue === "newer"
    ? "Saved by a newer version of Codemux. Update Codemux to use it. Nothing was changed."
    : "Codemux could not read this plugin's saved record, so it was left untouched.";
}

/** A read-only record's version reads 0.0.0 when the saved one was unreadable. */
export function pluginDisplayVersion(installation: PluginInstallation): string | null {
  if (!isPluginRecordReadOnly(installation)) return installation.manifest.version;
  return installation.manifest.version === "0.0.0" ? null : installation.manifest.version;
}

/** Derived compatibility wins over a persisted status written by another version. */
export const isPluginUnavailable = (installation: PluginInstallation) =>
  installation.compatibility === undefined
    ? installation.status === "incompatible"
    : !installation.compatibility.available;

export function pluginStatusPresentation(
  installation: PluginInstallation,
  /** Safe mode: nothing runs, so nothing claims to be ready or offers a restart. */
  options: { readonly executionDisabled?: boolean | undefined } = {},
): PluginStatusPresentation {
  if (isPluginRecordReadOnly(installation)) return { label: "Read-only", tone: "muted", primary: "none" };
  if (isPluginUnavailable(installation)) {
    // The switch keeps showing what the user chose; this says why it is not running.
    return installation.enabled
      ? { label: "Unavailable", tone: "error", primary: "none" }
      : { label: "Off · unavailable here", tone: "muted", primary: "none" };
  }
  if (installation.status === "needs-consent" || installation.consentedDigest !== installation.digest) {
    return {
      label: installation.consentedDigest === null ? "Needs review" : "Changed, needs review",
      tone: "warning",
      primary: "review",
    };
  }
  if (options.executionDisabled === true) {
    return installation.enabled
      ? { label: "Paused · plugin code off", tone: "muted", primary: "none" }
      : { label: "Off", tone: "muted", primary: "none" };
  }
  switch (installation.status) {
    case "disabled":
      return { label: "Off", tone: "muted", primary: "none" };
    case "idle":
      return { label: "Ready", tone: "success", primary: "none" };
    case "running":
      return { label: "Running", tone: "success", primary: "none" };
    case "error":
      return { label: "Error", tone: "error", primary: "resume" };
    case "quarantined":
      return { label: "Stopped after repeated failures", tone: "error", primary: "resume" };
    case "incompatible":
    default:
      // Persisted by another version (or a state this one does not know): nothing runs until it is understood.
      return { label: "Not running", tone: "muted", primary: "none" };
  }
}

/** Background event delivery paused. Actions keep working meanwhile. */
export function pluginObserverError(installation: PluginInstallation): string | null {
  const error = installation.observerError;
  return typeof error === "string" && error.trim() !== "" ? error : null;
}

// ── attention and recovery ────────────────────────────────────────────

export type PluginAttentionKind = "unavailable" | "fault" | "events-paused" | "read-only" | "other";

export interface PluginAttention {
  readonly installationId: string;
  readonly name: string;
  readonly kind: PluginAttentionKind;
  readonly title: string;
  readonly message: string;
  readonly since: string;
  /** Changes whenever a new problem is reported, so dismissing an old one never hides it. */
  readonly key: string;
  /** A plugin the user turned off only nags about real failures. */
  readonly notify: boolean;
}

const ATTENTION_TITLES: Record<PluginAttentionKind, string> = {
  unavailable: "Unavailable in this version of Codemux",
  fault: "Stopped after a failure",
  "events-paused": "Background events paused",
  "read-only": "Read-only record",
  other: "Needs attention",
};

const attentionKind = (kind: string): PluginAttentionKind =>
  kind in ATTENTION_TITLES ? (kind as PluginAttentionKind) : "other";

function derivedAttention(
  installation: PluginInstallation,
): { kind: PluginAttentionKind; message: string } | null {
  const recordIssue = pluginRecordIssueMessage(installation);
  if (recordIssue !== null) return { kind: "read-only", message: recordIssue };
  if (isPluginUnavailable(installation)) {
    const missing = [
      ...(installation.compatibility?.missingCapabilities ?? []),
      ...(installation.compatibility?.unsupportedRequirements ?? []),
    ];
    return {
      kind: "unavailable",
      message:
        installation.error ??
        (missing.length > 0
          ? `Needs ${missing.join(", ")}, which this version of Codemux does not support.`
          : "Needs something this version of Codemux does not support."),
    };
  }
  if (installation.status === "error" || installation.status === "quarantined") {
    return {
      kind: "fault",
      message:
        installation.error ??
        (installation.status === "quarantined"
          ? "Stopped after repeated failures."
          : "The plugin stopped after an error."),
    };
  }
  const observerError = pluginObserverError(installation);
  return observerError === null ? null : { kind: "events-paused", message: observerError };
}

export function pluginAttention(installation: PluginInstallation): PluginAttention | null {
  // Safe mode stops every plugin at once; the page notice says so, not each row.
  const reported = installation.attention?.kind === "safe-mode" ? undefined : installation.attention;
  const attention = reported
    ? { kind: attentionKind(reported.kind), message: reported.message, since: reported.since }
    : (() => {
        const derived = derivedAttention(installation);
        return derived === null ? null : { ...derived, since: installation.updatedAt };
      })();
  if (attention === null) return null;
  // A plugin that is off observes nothing; the missed-runs choice comes when it is turned on.
  if (attention.kind === "events-paused" && !installation.enabled) return null;
  return {
    installationId: installation.installationId,
    name: installation.manifest.name,
    kind: attention.kind,
    title: ATTENTION_TITLES[attention.kind],
    message: attention.message,
    since: attention.since,
    key: `${installation.installationId}:${attention.kind}:${attention.since}`,
    notify: installation.enabled || attention.kind !== "unavailable",
  };
}

export interface PluginAttentionNotice {
  readonly key: string;
  readonly tone: "warning" | "error";
  readonly title: string;
  readonly description: string;
  readonly entries: ReadonlyArray<PluginAttention>;
}

/** One grouped notice, so an update that breaks several plugins says it once. */
export function pluginAttentionNotice(installations: ReadonlyArray<PluginInstallation>): PluginAttentionNotice | null {
  const entries = installations
    .map(pluginAttention)
    .filter((entry): entry is PluginAttention => entry !== null && entry.notify);
  const [first] = entries;
  if (first === undefined) return null;
  return {
    key: entries
      .map((entry) => entry.key)
      .sort()
      .join("|"),
    tone: entries.every((entry) => entry.kind === "events-paused") ? "warning" : "error",
    title: entries.length === 1 ? `${first.name} needs attention` : `${entries.length} plugins need attention`,
    description: entries.map((entry) => `${entry.name}: ${entry.title}.`).join(" "),
    entries,
  };
}

export interface PluginDetailRow {
  readonly label: string;
  readonly value: string;
  /** Paths, digests and identifiers read better monospaced and must be selectable. */
  readonly code?: boolean;
}

const runtimeLabel = (observation: { readonly hostVersion: string; readonly nodeVersion: string }) =>
  `Codemux ${observation.hostVersion} · runtime ${observation.nodeVersion}`;

const isLater = (left: string, right: string) => Date.parse(left) > Date.parse(right);

/**
 * The facts behind a plugin's state. Versions are shown as recorded; nothing
 * here claims which update caused a failure.
 */
export function pluginRecoveryDetails(
  installation: PluginInstallation,
  formatTime: (iso: string) => string,
): ReadonlyArray<PluginDetailRow> {
  if (isPluginRecordReadOnly(installation)) return [];
  const rows: PluginDetailRow[] = [];
  const attention = pluginAttention(installation);
  if (attention !== null) rows.push({ label: attention.title, value: attention.message });
  const missing = installation.compatibility?.missingCapabilities ?? [];
  if (missing.length > 0) rows.push({ label: "Not supported here", value: missing.join(", "), code: true });
  const fault = installation.health?.fault;
  const lastHealthy = installation.health?.lastHealthy;
  if (fault !== undefined) {
    if (fault.message !== attention?.message) rows.push({ label: "Last recorded failure", value: fault.message });
    if (fault.handler !== undefined) rows.push({ label: "Failed in", value: fault.handler, code: true });
    rows.push({ label: "When it failed", value: `${formatTime(fault.at)} · ${runtimeLabel(fault)}` });
  }
  if (lastHealthy !== undefined) {
    rows.push({
      label: fault !== undefined && isLater(lastHealthy.at, fault.at) ? "Ran normally after that" : "Last ran normally",
      value: `${formatTime(lastHealthy.at)} · ${runtimeLabel(lastHealthy)}`,
    });
  }
  const recovery = installation.recovery;
  if (recovery !== undefined) {
    rows.push({
      label: "Active revision",
      value:
        recovery.activeSource === "local-repair"
          ? `Your local fix, v${installation.manifest.version}`
          : `Publisher version, v${installation.manifest.version}`,
    });
    if (recovery.activeSource === "local-repair" && recovery.upstreamSource !== undefined) {
      rows.push({ label: "Publisher source", value: pluginSourceLabel(recovery.upstreamSource), code: true });
    }
    if (recovery.previous !== undefined) rows.push({ label: "Previous version kept", value: `v${recovery.previous.version}` });
    if (recovery.repair !== undefined) rows.push({ label: "Repair folder", value: recovery.repair.directory, code: true });
  }
  return rows;
}

/** What happens to the installed revision while a staged one waits for review. */
export function pluginCurrentRevisionNote(installation: PluginInstallation): string {
  const current =
    installation.recovery?.activeSource === "local-repair"
      ? "Your local fix"
      : `Version ${installation.manifest.version}`;
  return isPluginActive(installation)
    ? `${current} keeps running until you approve.`
    : `${current} stays selected until you approve.`;
}

/** Why an installed plugin's code is not offered right now. Safe mode is explained before this. */
export function pluginInactiveReason(installation: PluginInstallation): string | null {
  const recordIssue = pluginRecordIssueMessage(installation);
  if (recordIssue !== null) return recordIssue;
  if (isPluginActive(installation)) return null;
  const attention = pluginAttention(installation);
  const explained = (fallback: string) =>
    attention !== null && attention.kind !== "events-paused" ? `${attention.title}: ${attention.message}` : fallback;
  if (isPluginUnavailable(installation)) return explained("Unavailable in this version of Codemux.");
  if (installation.consentedDigest !== installation.digest) {
    return installation.consentedDigest === null
      ? "The plugin needs review before it can run."
      : "The plugin's files changed. Review them before it can run.";
  }
  if (!installation.enabled) return "The plugin is turned off.";
  return explained("The plugin is not running.");
}

const withoutTrailingSeparators = (directory: string) => directory.replace(/[\\/]+$/, "");

/** Whether the repair folder was copied from what runs now (or the running fix came from it). */
function pluginRepairWorkspaceCurrent(
  installation: PluginInstallation,
  repair: { readonly baseDigest: string; readonly directory: string },
): boolean {
  if (repair.baseDigest === installation.digest) return true;
  return (
    installation.recovery?.activeSource === "local-repair" &&
    installation.source.kind === "local" &&
    withoutTrailingSeparators(installation.source.directory) === withoutTrailingSeparators(repair.directory)
  );
}

export interface PluginRecoveryActions {
  /** Copy the running files for an agent, or reopen the folder made earlier. */
  readonly repair: "prepare" | "open" | null;
  /** Stage the repair folder's current files for review. */
  readonly stageRepair: boolean;
  /** Stage the kept previous version; it runs with today's data. */
  readonly usePrevious: { readonly version: string; readonly digest: string } | null;
  readonly runningLocalFix: boolean;
}

const NO_RECOVERY: PluginRecoveryActions = {
  repair: null,
  stageRepair: false,
  usePrevious: null,
  runningLocalFix: false,
};

export function pluginRecoveryActions(
  installation: PluginInstallation,
  recoverySupported: boolean,
): PluginRecoveryActions {
  if (!recoverySupported || isPluginRecordReadOnly(installation)) return NO_RECOVERY;
  const recovery = installation.recovery;
  const pendingKind = installation.pendingUpdate?.kind;
  const previous = recovery?.previous;
  const repairCurrent = recovery?.repair !== undefined && pluginRepairWorkspaceCurrent(installation, recovery.repair);
  return {
    repair: repairCurrent ? "open" : "prepare",
    stageRepair: repairCurrent && pendingKind !== "local-repair",
    usePrevious:
      previous !== undefined && pendingKind !== "previous"
        ? { version: previous.version, digest: previous.digest }
        : null,
    runningLocalFix: recovery?.activeSource === "local-repair",
  };
}

export type PluginBacklogDecision =
  | { readonly kind: "none" }
  /** Nothing was missed, but the host still wants the gap closed explicitly. */
  | { readonly kind: "resolved"; readonly eventBacklog: PluginEventBacklogPolicy }
  | { readonly kind: "choose"; readonly count: number; readonly throughSequence: number };

/** Missed runs are never delivered in a surprise burst: the user picks deliver or skip. */
export function pluginBacklogDecision(backlog: PluginEventBacklog): PluginBacklogDecision {
  if (!backlog.requiresChoice) return { kind: "none" };
  if (backlog.count === 0) {
    return { kind: "resolved", eventBacklog: { policy: "skip", throughSequence: backlog.throughSequence } };
  }
  return { kind: "choose", count: backlog.count, throughSequence: backlog.throughSequence };
}

/**
 * Which request of an approval carries the missed-runs choice: consent of an
 * enabled plugin, or the enable that follows re-approving a disabled one. A
 * first approval starts from new runs; approving a staged revision leaves a
 * disabled plugin off.
 */
export function pluginApprovalBacklogStep(installation: PluginInstallation): "consent" | "enable" | null {
  if (installation.consentedDigest === null) return null;
  if (installation.enabled) return "consent";
  return installation.pendingUpdate === undefined ? "enable" : null;
}

const PLUGIN_STOPPED_REASONS: ReadonlySet<string> = new Set([
  "incompatible",
  "start_failed",
  "crashed",
  "timeout",
  "quarantined",
  "disabled",
  "needs_consent",
]);

/** The request failed because of the plugin's state, not its input; Settings explains it. */
export const pluginFailureNeedsDetails = (failure: PluginFailure) => PLUGIN_STOPPED_REASONS.has(failure.reason);

/** Repair folders hold the user's edits, so removing never deletes them. */
export function pluginRemovalNote(installation: PluginInstallation): string {
  const repair = installation.recovery?.repair;
  return [
    "Removing doesn't undo anything it already did, in Codemux or outside it.",
    repair === undefined ? undefined : `Your repair folder stays at ${repair.directory}.`,
  ]
    .filter(Boolean)
    .join(" ");
}

// ── sources and settings ──────────────────────────────────────────────

type PluginSourceParseResult =
  | { readonly ok: true; readonly source: PluginSource }
  | { readonly ok: false; readonly message: string };

/**
 * An absolute folder, or an https Git URL with an optional `#ref`. The host
 * validates again; this only gives quick feedback.
 */
export function parsePluginSourceInput(raw: string): PluginSourceParseResult {
  const value = raw.trim();
  if (value === "") return { ok: false, message: "Enter a folder path or Git URL." };
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(value)) {
    let url: URL;
    try {
      url = new URL(value);
    } catch {
      return { ok: false, message: "That URL is not valid." };
    }
    if (url.protocol !== "https:") return { ok: false, message: "Git sources must use https." };
    if (url.username !== "" || url.password !== "") return { ok: false, message: "Remove credentials from the URL." };
    let ref: string;
    try {
      ref = decodeURIComponent(url.hash.slice(1)).trim();
    } catch {
      return { ok: false, message: "The ref after # has an invalid % escape." };
    }
    url.hash = "";
    return { ok: true, source: { kind: "git", url: url.toString(), ...(ref === "" ? {} : { ref }) } };
  }
  if (value.startsWith("/") || /^[a-zA-Z]:[\\/]/.test(value) || value.startsWith("\\\\")) {
    return { ok: true, source: { kind: "local", directory: value } };
  }
  return { ok: false, message: "Use an absolute folder path, or an https Git URL." };
}

export function pluginSourceLabel(source: PluginSource): string {
  if (source.kind === "local") return source.directory;
  return source.ref === undefined ? source.url : `${source.url}#${source.ref}`;
}

export type PluginSettingDraft = Readonly<Record<string, string | boolean>>;

/** Secrets never come back from the host, so their draft always starts empty. */
export function pluginSettingsDraft(installation: PluginInstallation): PluginSettingDraft {
  const draft: Record<string, string | boolean> = {};
  for (const setting of installation.manifest.settings) {
    const value = installation.settings[setting.key] ?? setting.default;
    if (setting.type === "boolean") draft[setting.key] = value === true;
    else if (setting.type === "secret") draft[setting.key] = "";
    else draft[setting.key] = value === undefined ? "" : String(value);
  }
  return draft;
}

type PluginSettingsPatchResult =
  | { readonly ok: true; readonly values: Record<string, PluginSettingValue> }
  | { readonly ok: false; readonly errors: Readonly<Record<string, string>> };

/**
 * Only changed values are sent. An empty secret field keeps the stored secret;
 * a key in `clearedSecrets` sends "" so the host deletes it.
 */
export function buildPluginSettingsPatch(
  settings: ReadonlyArray<PluginSetting>,
  current: Readonly<Record<string, PluginSettingValue>>,
  draft: PluginSettingDraft,
  clearedSecrets: ReadonlySet<string> = new Set(),
): PluginSettingsPatchResult {
  const values: Record<string, PluginSettingValue> = {};
  const errors: Record<string, string> = {};
  for (const setting of settings) {
    const raw = draft[setting.key];
    const previous = current[setting.key] ?? setting.default;
    switch (setting.type) {
      case "boolean": {
        const next = raw === true;
        if (next !== (previous === true)) values[setting.key] = next;
        break;
      }
      case "number": {
        const text = typeof raw === "string" ? raw.trim() : "";
        if (text === "") {
          if (previous !== undefined) errors[setting.key] = "Enter a number.";
          break;
        }
        const next = Number(text);
        if (!Number.isFinite(next)) {
          errors[setting.key] = "Enter a number.";
          break;
        }
        if (next !== previous) values[setting.key] = next;
        break;
      }
      case "string": {
        const next = typeof raw === "string" ? raw : "";
        if (next !== (previous === undefined ? "" : String(previous))) values[setting.key] = next;
        break;
      }
      case "secret": {
        const next = typeof raw === "string" ? raw : "";
        if (next !== "") values[setting.key] = next;
        else if (clearedSecrets.has(setting.key)) values[setting.key] = "";
        break;
      }
    }
  }
  return Object.keys(errors).length > 0 ? { ok: false, errors } : { ok: true, values };
}

/** A failure's readable parts, whatever shape the transport rejected with. */
export function pluginFailure(error: unknown): PluginFailure {
  if (typeof error === "object" && error !== null) {
    const record = error as Record<string, unknown>;
    const message = typeof record.message === "string" && record.message.trim() !== "" ? record.message : null;
    if (message !== null) {
      return {
        reason: typeof record.reason === "string" ? record.reason : "operation_failed",
        message,
        ...(typeof record.code === "string" ? { code: record.code } : {}),
      };
    }
  }
  if (typeof error === "string" && error.trim() !== "") {
    // A Tauri command rejects with a plain string; a JSON one may carry the reason.
    try {
      const parsed: unknown = JSON.parse(error);
      if (typeof parsed === "object" && parsed !== null && "message" in parsed) return pluginFailure(parsed);
    } catch {
      // Not JSON: the string is the message, maybe after a `reason: ` prefix.
    }
    const prefixed = /^([a-z][a-z_]{1,63}): ([\s\S]+)$/.exec(error.trim());
    if (prefixed) return { reason: prefixed[1]!, message: prefixed[2]! };
    return { reason: "operation_failed", message: error };
  }
  return { reason: "operation_failed", message: "The plugin request failed." };
}
