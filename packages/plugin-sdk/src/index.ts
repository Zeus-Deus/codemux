/** Adapted from T3 Tools' MIT author contract; see THIRD_PARTY_NOTICES.md.
 * Public author API. Capability-gated additions require host discovery. Plugins run as the host's OS user; process isolation is not an OS sandbox. */
export const apiVersion = 1;
export type JsonValue =
  | null
  | boolean
  | number
  | string
  | readonly JsonValue[]
  | { readonly [key: string]: JsonValue };
export type PluginCapability =
  | "storage"
  | "settings"
  | "threads.read"
  | "threads.send"
  | "threads.create"
  | "events.run-finalized"
  | "ui"
  | "ui.sidebar-views"
  | "ui.native-trees"
  | "ui.conversation-cards"
  | "ui.changes"
  | "worktrees.create"
  | "workflow.query";
export type PluginSettingValue = string | number | boolean;
export interface PluginManifest {
  readonly id: string;
  readonly name: string;
  readonly description?: string;
  readonly version: string;
  readonly apiVersion: 1;
  readonly entry: string;
  readonly capabilities?: readonly string[];
  /** Unavailable optional capabilities are omitted from context.capabilities. */
  readonly optionalCapabilities?: readonly string[];
  /** Additional required capabilities; both required lists are fail-closed. */
  readonly requiredCapabilities?: readonly string[];
  readonly requires?: {
    readonly apiVersion?: number;
    readonly hostVersion?: string;
    readonly codemux?: string;
    readonly capabilities?: readonly string[];
  };
  readonly actions?: readonly {
    readonly id: string;
    readonly title: string;
    readonly description?: string;
    readonly placements?: readonly ("palette" | "thread-menu" | "message-selection")[];
    readonly agentCallable?: boolean;
  }[];
  readonly views?: readonly {
    readonly id: string;
    readonly title: string;
    readonly entry: string;
    readonly placement: "page" | "side-panel" | "conversation";
  }[];
  /** Compact statuses shown with the open thread. Register `status:<id>` for each. */
  readonly threadStatus?: readonly {
    readonly id: string;
    readonly title: string;
    /** A declared side-panel view opened when the person activates the status. */
    readonly viewId?: string;
  }[];
  /**
   * Declarative sidebar rules, requiring `ui.sidebar-views`. Client support can vary;
   * use a nativeTrees contribution when an executable metadata-driven tree is needed.
   */
  readonly sidebarViews?: readonly {
    readonly id: string;
    readonly title: string;
    readonly description?: string;
    readonly groupBy: "project";
    /** Projects with nothing active and no activity for this many days (1-365) fold away. */
    readonly quietAfterDays?: number;
    /** Whether projects without threads get a heading. Defaults to `hide`. */
    readonly emptyProjects?: "show" | "hide";
  }[];
  readonly nativeTrees?: readonly {
    readonly id: string;
    readonly title: string;
    readonly description?: string;
  }[];
  readonly settings?: readonly PluginSetting[];
}
export type PluginSetting = {
  readonly key: string;
  readonly label: string;
  readonly description?: string;
} & (
  | { readonly type: "string"; readonly default?: string }
  | { readonly type: "number"; readonly default?: number }
  | { readonly type: "boolean"; readonly default?: boolean }
  | { readonly type: "secret"; readonly default?: never }
);
export interface PluginActionContext {
  readonly projectId?: string;
  readonly threadId?: string;
  readonly messageId?: string;
  readonly selection?: string;
}
export interface PluginInvocationContext extends PluginActionContext {
  /** Host-derived first non-subagent ancestor of the target; forks stop the chain. Not identity. */
  readonly subagentOf?: string;
  /**
   * A read failed or ancestry is missing, deleted, cyclic or over the depth bound; `subagentOf`
   * is omitted. Retry before choosing an owner from `threadId`. Optional in v1; older hosts omit
   * it, so absence alone does not prove a root. Retrying may not resolve deleted ancestry; ask the person before choosing an owner.
   */
  readonly lineage?: "unresolved";
}
export type PluginClientIntent =
  | { readonly type: "composer.insert"; readonly text: string; readonly quote?: boolean }
  | { readonly type: "preview.open"; readonly url: string }
  | { readonly type: "thread.reveal"; readonly threadId: string; readonly messageId?: string }
  | { readonly type: "view.open"; readonly viewId: string };
/** An image file the plugin wrote under `context.dataDirectory`, named by its relative path. */
export interface PluginResultImage {
  readonly path: string;
  readonly mimeType: "image/png" | "image/jpeg";
}
export interface PluginConversationCard {
  readonly viewId: string;
  /** Historical plain text proposal, at most 1000 characters. */
  readonly summary: string;
  /** Small JSON reference (e.g. planId), at most 2048 UTF-8 bytes. Full state lives in storage. */
  readonly data?: JsonValue;
}
export interface PluginCardReference extends PluginConversationCard {
  readonly installationId: string;
  readonly generation: string;
  readonly dataOwner: string;
}
export type PluginTreeNode =
  | {
      readonly id: string;
      readonly parentId?: string;
      readonly collapsedByDefault?: boolean;
      readonly kind: "group";
      readonly title: string;
    }
  | {
      readonly id: string;
      readonly parentId?: string;
      readonly collapsedByDefault?: boolean;
      readonly kind: "project";
      readonly projectId: string;
    }
  | {
      readonly id: string;
      readonly parentId?: string;
      /** Applies to group and project headings; native thread rows remain expanded. */
      readonly collapsedByDefault?: boolean;
      readonly kind: "thread";
      readonly threadId: string;
    };
export interface PluginTreeSnapshot {
  readonly nodes: readonly PluginTreeNode[];
}
export interface PluginModelSelection {
  readonly instanceId: string;
  readonly model: string;
  readonly options?: readonly { readonly id: string; readonly value: string | boolean }[];
}
/** Accepted input compatibility shape; host metadata uses canonical option selections. */
export interface PluginModelSelectionInput extends Omit<PluginModelSelection, "options"> {
  readonly options?: PluginModelSelection["options"] | Readonly<Record<string, string | boolean>>;
}
/** Missing native configuration/model stays null instead of inventing a current selection. */
export interface PluginStoredModelSelection extends Omit<PluginModelSelection, "instanceId" | "model"> {
  readonly instanceId: string | null;
  readonly model: string | null;
}
export type PluginRuntimeMode = "approval-required" | "auto-accept-edits" | "full-access";
export interface PluginThreadCreateInput {
  readonly projectId: string;
  /** Native create titles are at most 100 UTF-8 bytes. */
  readonly title?: string;
  /** Nonempty prompt, at most 64,000 UTF-8 bytes. */
  readonly text: string;
  readonly idempotencyKey: string;
  readonly modelSelection?: PluginModelSelectionInput;
  readonly runtimeMode?: PluginRuntimeMode;
  readonly interactionMode?: "default" | "plan";
  /** Exact advertised provider mode; use this when portable runtimeMode has no mapping. */
  readonly nativePermissionMode?: string;
  readonly workspaceId?: string;
  readonly workspaceStrategy?:
    | { readonly type: "root" }
    | {
        readonly type: "worktree";
        readonly baseRef: string;
        readonly branch?: string;
        readonly startFromOrigin?: boolean;
      };
}
export interface PluginPageInput {
  readonly cursor?: number;
  readonly limit?: number;
}
export interface PluginProjectMetadata {
  readonly id: string;
  readonly title: string;
  readonly workspaceRoot: string;
}
export interface PluginThreadMetadata {
  /** Host-derived creation origin, for display grouping only. */
  readonly creationSource?: string;
  readonly isRunning?: boolean;
  readonly needsAttention?: boolean;
  readonly id: string;
  readonly projectId: string;
  readonly title: string | null;
  readonly workspaceId: string;
  readonly importedSnapshot: boolean;
  readonly nativePermissionMode: string | null;
  readonly modelSelection: PluginStoredModelSelection;
  readonly runtimeMode: PluginRuntimeMode | null;
  readonly interactionMode: "default" | "plan";
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly archivedAt: number | null;
  readonly settledAt: string | null;
  readonly settledOverride: "settled" | "active" | null;
  readonly snoozedUntil: string | null;
  readonly snoozedAt: string | null;
  readonly pinnedAt: string | null;
  readonly branch: string | null;
  readonly worktreePath: string | null;
  readonly lineage: {
    readonly parentThreadId: string | null;
    readonly relationshipToParent: "fork" | "subagent" | null;
    readonly rootThreadId: string;
  };
  readonly launchedFrom?: { readonly threadId: string };
}
export type PluginModelOption = {
  readonly id: string;
  readonly label: string;
  readonly description?: string;
} & (
  | { readonly type: "boolean"; readonly currentValue?: boolean }
  | {
      readonly type: "select";
      readonly currentValue?: string;
      readonly promptInjectedValues?: readonly string[];
      readonly options: readonly {
        readonly id: string;
        readonly label: string;
        readonly description?: string;
        readonly isDefault?: boolean;
      }[];
    }
);
export interface PluginModel {
  readonly modelSelection: PluginModelSelection;
  readonly title: string | null;
  readonly providerTitle: string;
  readonly options: readonly PluginModelOption[];
  /** Native provider catalog details; additive fields are preserved. */
  readonly nativeCapabilities: {
    readonly id: string;
    readonly label: string;
    readonly effort_levels: readonly string[];
    readonly default_effort: string | null;
    readonly prompt_injected_effort_levels: readonly string[];
    readonly context_window_options: readonly { readonly value: string; readonly label: string; readonly is_default: boolean; readonly context_window_tokens: number | null }[];
    readonly supports_fast_mode: boolean;
    readonly service_tiers: readonly { readonly value: string; readonly label: string; readonly description: string }[];
    readonly [key: string]: unknown;
  };
  readonly permissionModes: readonly string[];
}
export interface PluginGrants {
  readonly capabilities: readonly PluginCapability[];
  readonly projectIds: readonly string[];
}
export interface PluginCompatibility {
  readonly available: boolean;
  readonly apiVersion: number;
  readonly missingCapabilities: readonly string[];
  readonly unsupportedOptionalCapabilities: readonly string[];
  readonly unsupportedRequirements: readonly string[];
}
/** Native receipt: succeeded means command admission/delivery, not completed agent work. */
export interface PluginOperationReceipt {
  readonly operation_id: string;
  readonly tool: string;
  readonly state: "accepted" | "running" | "succeeded" | "failed" | "needs_attention" | (string & {});
  readonly workspace_id: string | null;
  readonly thread_id: string | null;
  readonly result: JsonValue;
  readonly error: JsonValue;
  readonly delivery_id: string | null;
  readonly dispatch_disposition: string | null;
  readonly created_at: string;
  readonly updated_at: string;
}
export interface PluginWorkspaceMetadata {
  readonly id: string;
  readonly projectId: string;
  readonly title: string;
  readonly cwd: string;
  readonly projectRoot: string;
  readonly branch: string | null;
  readonly worktreePath: string | null;
  readonly archivedAt: number | null;
}
export interface PluginProjectPage { readonly projects: readonly PluginProjectMetadata[]; readonly nextCursor: number | null; }
export interface PluginWorkspacePage { readonly workspaces: readonly PluginWorkspaceMetadata[]; readonly nextCursor: number | null; }
export interface PluginThreadPage { readonly threads: readonly PluginThreadMetadata[]; readonly nextCursor: number | null; }
export interface PluginModelPage { readonly models: readonly PluginModel[]; readonly nextCursor: number | null; }
export interface PluginWorkflowQuery {
  (input: PluginPageInput & { readonly query: "projects" }): Promise<PluginProjectPage>;
  (input: PluginPageInput & { readonly query: "workspaces" }): Promise<PluginWorkspacePage>;
  (input: PluginPageInput & { readonly query: "threads"; readonly projectId: string }): Promise<PluginThreadPage>;
  (input: { readonly query: "metadata"; readonly threadId: string }): Promise<PluginThreadMetadata>;
  (input: PluginPageInput & { readonly query: "models"; readonly projectId: string }): Promise<PluginModelPage>;
  /** Reads an earlier receipt owned by this installation + current authenticated principal. */
  (input: { readonly query: "operation"; readonly operationId: string }): Promise<PluginOperationReceipt>;
  /** Reads a mutation by its key within this invocation's stable operation identity. */
  (input: { readonly query: "effect"; readonly idempotencyKey: string }): Promise<PluginOperationReceipt>;
  (input: { readonly query: "thread-status"; readonly threadId: string }): Promise<PluginNativeThreadStatus>;
}
export interface PluginNativeThreadStatus {
  readonly threadId: string;
  readonly projectId: string;
  readonly status: {
    readonly workspace_id: string;
    readonly thread_id: string;
    readonly phase: string;
    readonly settled: boolean;
    readonly runtime_live: boolean;
    readonly turn_id: string | null;
    readonly queued_ids: readonly string[];
    readonly uncertain_queued_ids: readonly string[];
    readonly pending_approvals: readonly JsonValue[];
    readonly last_turn: JsonValue;
    readonly deliveries_needing_attention: readonly JsonValue[];
    readonly imported_snapshot: boolean;
    readonly [key: string]: unknown;
  };
}
export interface PluginInvokeResult {
  readonly card?: PluginConversationCard;
  readonly message?: string;
  readonly data?: unknown;
  readonly intents?: readonly PluginClientIntent[];
  /**
   * Up to four PNG or JPEG files, each at most 4 MiB. The host reads and verifies them after the
   * handler returns; agents receive them as image content and views as base64 `data`.
   */
  readonly images?: readonly PluginResultImage[];
  /** Host-reported presentation requests this host or client could not interpret. */
  readonly unsupportedIntentCount?: number;
}
/**
 * Who started the invocation, established by the host. `threadId` is present only for an agent the
 * host verified is acting for that Codemux thread. `context` is only the requested target: use `caller`,
 * not `context.threadId`, to decide what an agent owns.
 */
export interface PluginCaller {
  readonly kind: "client" | "agent" | "background";
  readonly threadId?: string;
  /** Host-derived first non-subagent ancestor of this agent's thread; forks stop the chain. */
  readonly subagentOf?: string;
  /**
   * A read failed or ancestry is missing, deleted, cyclic or over the depth bound; `subagentOf`
   * is omitted. Retry before choosing an owner from `threadId`. Optional in v1; older hosts omit
   * it, so absence alone does not prove a root. Retrying may not resolve deleted ancestry; ask the person before choosing an owner.
   */
  readonly lineage?: "unresolved";
}
export interface PluginInvocation {
  readonly signal: AbortSignal;
  readonly operationId: string;
  readonly caller: PluginCaller;
  readonly context?: PluginInvocationContext;
}
/**
 * What a `status:<id>` handler returns as `data`, or `null` to show nothing. Clients ask while
 * the thread is visible and may show the last answer for a short while, so keep it cheap and
 * describe current state, not a transient event.
 */
export interface PluginThreadStatus {
  readonly label: string;
  readonly tone: "neutral" | "info" | "success" | "warning" | "danger";
  readonly detail?: string;
}
export interface PluginRunFinalizedEvent {
  /** Delivery is at least once. Persist this id before producing a non-idempotent external effect. */
  readonly deliveryId: string;
  readonly threadId: string;
  readonly projectId: string;
  readonly runId: string;
  readonly outcome: "completed" | "failed" | "interrupted";
  readonly summary: string;
  readonly sequence: number;
  /** False for legacy/unverified input provenance: observation does not authorize automation. */
  readonly effectsAllowed: boolean;
}
/** Host call failures preserve these fields on the Error thrown to a plugin handler. */
export interface PluginHostError extends Error {
  readonly reason: string;
  /** `idempotency_conflict` means this operation key is already bound to a different request. */
  readonly code?: string;
  /** Present only when the current caller still has authority to see the earlier effect. */
  readonly priorEffect?: JsonValue;
}
export interface PluginThread {
  readonly threadId: string;
  readonly projectId: string;
  readonly title: string | null;
  readonly nextCursor: number | null;
  readonly totalVisibleMessages: number;
  readonly messages: readonly {
    readonly id: number;
    readonly role: string;
    readonly text: string;
    readonly createdAt: string;
  }[];
}
/** Await host calls before returning; invocation completion revokes outstanding calls. */
export type PluginHandler = (
  input: unknown,
  invocation: PluginInvocation,
) => Promise<PluginInvokeResult | void> | PluginInvokeResult | void;
export interface PluginContext {
  readonly apiVersion: 1;
  readonly capabilities: readonly PluginCapability[];
  /**
   * A private 0700 directory for this plugin's files on the server. It survives updates and
   * restarts, follows the same retention as `storage` on removal, and is deleted with its data.
   */
  readonly dataDirectory: string;
  readonly signal: AbortSignal;
  /** Register during activation. Host calls are available only inside a handler invocation. */
  readonly handle: (
    name:
      | `action:${string}`
      | `status:${string}`
      | `tree:${string}`
      | `view:${string}:${string}`
      | "event:run.finalized",
    handler: PluginHandler,
  ) => void;
  readonly log: (level: "debug" | "info" | "warn" | "error", message: string) => void;
  readonly settings: { readonly get: (key: string) => Promise<PluginSettingValue | null> };
  readonly secrets: { readonly get: (key: string) => Promise<string | null> };
  readonly storage: {
    readonly get: (key: string) => Promise<JsonValue>;
    readonly set: (key: string, value: JsonValue) => Promise<null>;
    readonly delete: (key: string) => Promise<null>;
  };
  readonly threads: {
    readonly read: (input: PluginPageInput & { readonly threadId: string }) => Promise<PluginThread>;
    /**
     * Keep this key stable across retries and revisions within the same invocation and caller
     * identity. Reusing that identity for a different request rejects with `idempotency_conflict`.
     * Equal operation IDs from different client sessions do not establish a replay.
     */
    readonly send: (input: {
      readonly threadId: string;
      readonly text: string;
      readonly idempotencyKey: string;
    }) => Promise<PluginOperationReceipt>;
    readonly create: (input: PluginThreadCreateInput) => Promise<PluginOperationReceipt>;
    readonly list: (input: PluginPageInput & { readonly projectId: string }) => Promise<{
      readonly threads: readonly PluginThreadMetadata[];
      readonly nextCursor: number | null;
    }>;
    readonly metadata: (input: { readonly threadId: string }) => Promise<PluginThreadMetadata>;
  };
  readonly projects: {
    readonly list: (input?: PluginPageInput) => Promise<{
      readonly projects: readonly PluginProjectMetadata[];
      readonly nextCursor: number | null;
    }>;
  };
  readonly workflow: { readonly query: PluginWorkflowQuery };
  readonly workspaces: {
    readonly list: (input?: PluginPageInput) => Promise<PluginWorkspacePage>;
  };
  readonly models: {
    readonly list: (
      input: PluginPageInput & { readonly projectId: string },
    ) => Promise<{ readonly models: readonly PluginModel[]; readonly nextCursor: number | null }>;
  };
  /** Call inside a handler after changing state; topics must name declared contributions. */
  readonly ui: {
    readonly invalidate: (
      topics: readonly (`tree:${string}` | `view:${string}` | `status:${string}`)[],
    ) => Promise<null>;
  };
  readonly trees: {
    readonly register: (
      id: string,
      handler: (invocation: PluginInvocation) => Promise<PluginTreeSnapshot> | PluginTreeSnapshot,
    ) => void;
  };
  readonly events: {
    readonly onRunFinalized: (
      handler: (
        event: PluginRunFinalizedEvent,
        invocation: PluginInvocation,
      ) => Promise<PluginInvokeResult | void> | PluginInvokeResult | void,
    ) => void;
  };
}
/** Host services share the current invocation's authority; this is a type alias, not ctx.host. */
export type PluginHost = Pick<PluginContext, "settings" | "secrets" | "storage" | "threads" | "projects" | "workspaces" | "models" | "workflow" | "ui">;
export interface PluginModule {
  readonly activate: (context: PluginContext) => Promise<void> | void;
  readonly deactivate?: () => Promise<void> | void;
}

/** HTML views use the injected window.codemux bridge, separate from the worker context. */
export type PluginViewFeature = "changes" | "local-intents" | "resize" | "card-context";
export interface PluginViewContext {
  readonly theme: "light" | "dark";
  readonly styles: Readonly<Record<string, string>>;
  readonly platform: "web" | "desktop" | "mobile";
  readonly placement: "page" | "side-panel" | "conversation";
  readonly view: { readonly installationId: string; readonly viewId: string };
  readonly thread: { readonly threadId: string; readonly projectId: string | null } | null;
  readonly features?: readonly PluginViewFeature[];
  readonly card?: PluginConversationCard;
}
export interface PluginViewBridge {
  readonly apiVersion: 1;
  readonly features: readonly PluginViewFeature[];
  readonly ready: Promise<PluginViewContext>;
  readonly context: PluginViewContext | null;
  readonly invoke: (name: string, input?: JsonValue) => Promise<PluginInvokeResult>;
  readonly onContext: (listener: (context: PluginViewContext) => void) => () => void;
  readonly onChanged?: (listener: (change: { readonly topic: string }) => void) => () => void;
  readonly intent?: (intent: PluginClientIntent) => Promise<void>;
  readonly resize?: (height: number) => void;
}
/** Discovery reflects supported AND granted capabilities, not just manifest requests. */
export function hasCapability(context: Pick<PluginContext, "capabilities">, capability: string): boolean {
  return context.capabilities.some(value => value === capability);
}
/** A native receipt never proves the agent's task completed, even in state succeeded. */
export function operationDisposition(receipt: Pick<PluginOperationReceipt, "state">): "pending" | "accepted" | "failed" | "uncertain" {
  switch (receipt.state) {
    case "accepted": case "running": return "pending";
    case "succeeded": return "accepted";
    case "failed": return "failed";
    default: return "uncertain";
  }
}
/** Historical text stays useful when a card's plugin is disabled, missing, or incompatible. */
export function conversationCardSummary(card: Pick<PluginConversationCard, "summary">): string {
  return card.summary;
}
