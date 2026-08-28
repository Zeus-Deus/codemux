/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi, beforeEach } from "vitest";
import { act, cleanup, fireEvent, render, waitFor, within } from "@testing-library/react";

let currentMessages: unknown[] = [];
// Overridable per-test: thread -> messages map so the new race-fix
// tests can observe which slice AgentChatPane subscribes to.
let currentThreadsMap: Record<string, unknown[]> = {};
// Per-thread slice field overrides (mode, permissionMode, etc.)
// used by the mode-pill removal tests to seed an "ask" or "plan"
// pill state before the X-button click.
type SliceOverrides = {
  mode?: "default" | "plan" | "ask" | "debug";
  modePriorPermissionMode?: string | null;
  permissionMode?: string;
  sessionLaunchMode?: string;
  model?: string | null;
  effort?: string | null;
  contextWindow?: string | null;
  fastMode?: boolean;
  resumeCursor?: Record<string, string> | null;
  hasDebugActivity?: boolean;
  debugActivityResolved?: boolean;
  /** Composer text — the Thread Scope deferred-worktree submit tests
   *  seed this so the pane's `draft` (slice.inputDraft) is non-empty. */
  inputDraft?: string;
  /** Live-turn flag. The send-anchor persistence test flips this to prove
   *  a turn settling leaves the anchor untouched. */
  streaming?: boolean;
  /** Durable resume cursor. `undefined` (the default) means "never
   *  hydrated", which sends the mount effect down the cold path. */
  lastPersistedEventId?: number | null;
};
let currentSliceOverrides: Record<string, SliceOverrides> = {};
let currentDraftsById: Record<
  string,
  {
    draftId: string;
    threadId: string;
    promotedTo: { workspaceId: string; paneId: string } | null;
    materializedTo?: { workspaceId: string; paneId: string; threadId: string } | null;
  }
> = {};
let workspaceIdForPaneOverride: string | null = "ws-home";
const setShowNewWorkspaceDialogMock = vi.fn();
const setActiveDraftMock = vi.fn();
// Hoisted so the model-seed effect tests can observe whether (and
// with what) setModel was called from inside AgentChatPane's mount.
// Stable across selector calls — the agent-chat-store mock below
// reuses this exact spy in every state object it produces.
const setModelMock = vi.fn();
// Same hoisting pattern for the mode-pill removal tests so they can
// assert which slice setters were called with which values.
const setModeMock = vi.fn();
const setModePriorMock = vi.fn();
const setPermissionModeMock = vi.fn();
const setInputDraftMock = vi.fn();
const markRequestResolvedMock = vi.fn();
const setHasDebugActivityMock = vi.fn();
const setDebugActivityResolvedMock = vi.fn();
// Hoisted so the mount-seed effect tests (design F) can assert which
// picker fields the pane restored from the persisted session row.
const setEffortMock = vi.fn();
const setContextWindowMock = vi.fn();
const setFastModeMock = vi.fn();
const setResumeCursorMock = vi.fn();
// Hoisted mock for the bug/chat-agent-empty regression test (an
// unmount/remount that lands on an existing thread should pull the
// persisted transcript and overlay it onto the in-memory slice when
// disk has more rendered messages than memory).
const hydrateThreadMock = vi.fn();
// Warm cursor resume: the tail merge and the cursor-invalidate escape
// hatch the mount effect uses when a read fails.
const applyPayloadsTailMock = vi.fn();
const invalidateThreadCursorMock = vi.fn();
// Mount registration keeps eviction off a displayed thread; the pane just
// has to call it and run the returned unregister on teardown.
const unregisterMountedThreadMock = vi.fn();
const registerMountedThreadMock = vi.fn(() => unregisterMountedThreadMock);
// Hoisted so the deferred-worktree submit tests can assert the
// optimistic user bubble was appended to the NEW worktree's thread.
const appendUserMessageMock = vi.fn();
/** Stable across `getState()` calls so a test can assert what the pane
 *  imperatively folded into the slice (e.g. Stop's local settle). */
const applyEventMock = vi.fn();

vi.mock("./ChatHomeLanding", () => ({
  ChatHomeLanding: ({ composer }: { composer: React.ReactNode }) => (
    <div data-testid="home-landing">{composer}</div>
  ),
}));

vi.mock("./ChatTranscript", () => ({
  ChatTranscript: ({
    messages,
    sessionStartedAt,
    sendAnchor,
    threadKey,
    onAcceptPlan,
    onRejectPlan,
    onEnterSubagent,
    onCancelQueued,
    onSendQueuedNow,
  }: {
    messages: unknown[];
    sessionStartedAt?: number;
    sendAnchor?: { clientNonce: string; nonce: number } | null;
    threadKey?: string | null;
    onAcceptPlan: (requestId: string) => void;
    onRejectPlan: (requestId: string) => void;
    onEnterSubagent?: (subagentId: string) => void;
    onCancelQueued?: (queuedId: string, text: string) => void;
    onSendQueuedNow?: (queuedId: string) => void;
  }) => (
    <div
      data-testid="transcript"
      data-message-count={messages.length}
      // The new-turn scroll contract's navigation intent. Empty string
      // encodes "no anchor" (nothing reserved / rolled back).
      data-send-anchor-nonce={sendAnchor ? String(sendAnchor.nonce) : ""}
      data-send-anchor-client-nonce={sendAnchor?.clientNonce ?? ""}
      data-thread-key={threadKey ?? ""}
      // Empty string encodes "no marker timestamp" (plain divider); a
      // numeric string is the parsed session `created_at` the pane wired
      // through for the D2 marker.
      data-session-started-at={
        sessionStartedAt == null ? "" : String(sessionStartedAt)
      }
    >
      <button
        data-testid="accept-plan"
        onClick={() => onAcceptPlan("req-1")}
      />
      <button
        data-testid="reject-plan"
        onClick={() => onRejectPlan("req-1")}
      />
      {/* Lets the viewMode-swap test trigger the pane's real
          onEnterSubagent handler without mounting the full card. */}
      <button
        data-testid="enter-subagent"
        onClick={() => onEnterSubagent?.("sub-1")}
      />
      {/* Dispatching a queued turn is a send, so it takes the same
          new-turn scroll contract as a composer submission. */}
      <button
        data-testid="cancel-queued"
        onClick={() => onCancelQueued?.("q-1", "queued follow-up")}
      />
      <button
        data-testid="send-queued-now"
        onClick={() => onSendQueuedNow?.("q-1")}
      />
    </div>
  ),
}));

vi.mock("./DebugCleanupBanner", () => ({
  DebugCleanupBanner: ({
    onCleanup,
    busy,
  }: {
    onCleanup: () => void;
    busy?: boolean;
  }) => (
    <button
      data-testid="debug-cleanup-banner"
      data-busy={busy ? "true" : "false"}
      onClick={onCleanup}
    />
  ),
}));

let lastDebugExitOpen = false;
let lastDebugExitOnChoose:
  | ((choice: "cleanup" | "leave" | "cancel") => void)
  | null = null;
vi.mock("./DebugExitDialog", () => ({
  DebugExitDialog: ({
    open,
    onChoose,
  }: {
    open: boolean;
    onChoose: (choice: "cleanup" | "leave" | "cancel") => void;
  }) => {
    lastDebugExitOpen = open;
    lastDebugExitOnChoose = onChoose;
    return open ? (
      <div data-testid="debug-exit-dialog">
        <button
          data-testid="debug-exit-cleanup"
          onClick={() => onChoose("cleanup")}
        />
        <button
          data-testid="debug-exit-leave"
          onClick={() => onChoose("leave")}
        />
        <button
          data-testid="debug-exit-cancel"
          onClick={() => onChoose("cancel")}
        />
      </div>
    ) : null;
  },
}));

vi.mock("./Composer", () => ({
  Composer: ({
    zone1Override,
    belowComposerSlot,
    topStripSlot,
    onSubmit,
    onStop,
    onContinueRun,
    onModeRemove,
    onModeActivate,
    onModelChange,
    onProviderModelChange,
    onContextWindowChange,
    onFastModeChange,
    onPermissionModeChange,
    provider,
    providerCliInstalled,
    providerAuthenticated,
    focusOnMount,
    configurationReady,
    sessionReady,
  }: {
    zone1Override?: React.ReactNode;
    belowComposerSlot?: React.ReactNode;
    topStripSlot?: React.ReactNode;
    onSubmit: () => void;
    onStop: () => void;
    onContinueRun?: () => void;
    onModeRemove: () => void;
    onModeActivate: (mode: "plan" | "ask" | "debug") => void;
    onModelChange: (model: string) => void;
    onProviderModelChange: (
      provider: "claude" | "codex" | "cursor" | "grok" | "opencode",
      model: string,
    ) => void;
    onContextWindowChange: (contextWindow: string) => void;
    onFastModeChange: (fastMode: boolean) => void;
    onPermissionModeChange: (mode: string) => void;
    provider: "claude" | "codex" | "cursor" | "grok" | "opencode";
    providerCliInstalled?: boolean | null;
    providerAuthenticated?: boolean | null;
    focusOnMount?: boolean;
    configurationReady?: boolean;
    sessionReady?: boolean;
  }) => (
    <div
      data-testid="composer"
      data-provider={provider}
      data-provider-cli-installed={String(providerCliInstalled)}
      data-provider-authenticated={String(providerAuthenticated)}
      data-focus-on-mount={focusOnMount ? "true" : "false"}
      data-configuration-ready={configurationReady ? "true" : "false"}
      data-session-ready={sessionReady ? "true" : "false"}
    >
      {/* The running-subagents strip is welded inside the real composer's
          top edge, so the mock has to render the slot for the pane's
          drill-in visibility rule to be observable. */}
      <div data-testid="composer-top-strip">{topStripSlot}</div>
      <div data-testid="zone1">{zone1Override}</div>
      <div data-testid="below-composer">{belowComposerSlot}</div>
      <button data-testid="composer-submit" onClick={() => onSubmit()} />
      <button data-testid="composer-stop" onClick={() => onStop()} />
      <button
        data-testid="composer-continue"
        onClick={() => onContinueRun?.()}
      />
      <button data-testid="mode-remove" onClick={() => onModeRemove()} />
      <button
        data-testid="mode-activate-plan"
        onClick={() => onModeActivate("plan")}
      />
      <button
        data-testid="mode-activate-ask"
        onClick={() => onModeActivate("ask")}
      />
      <button
        data-testid="mode-activate-debug"
        onClick={() => onModeActivate("debug")}
      />
      {/* Design G persist coverage: exercise the pane's real picker
          handlers so the test can assert the fire-and-forget
          `agentChatUpdateSessionConfig` write. */}
      <button
        data-testid="model-change"
        onClick={() => onModelChange("claude-sonnet-4-6")}
      />
      <button
        data-testid="grok-model-change"
        onClick={() => onModelChange("grok-future")}
      />
      <button
        data-testid="grok-model-change-a"
        onClick={() => onModelChange("grok-a")}
      />
      <button
        data-testid="grok-model-change-b"
        onClick={() => onModelChange("grok-b")}
      />
      <button
        data-testid="provider-model-change"
        onClick={() => onProviderModelChange("codex", "gpt-5.4")}
      />
      <button
        data-testid="context-window-change"
        onClick={() => onContextWindowChange("1m")}
      />
      <button
        data-testid="fast-mode-change"
        onClick={() => onFastModeChange(true)}
      />
      {/* Permission-mode picker selection. Per-turn providers have to
          push the choice onto the LIVE session; per-session ones
          restart. Both branches are asserted below. */}
      <button
        data-testid="permission-mode-change"
        onClick={() => onPermissionModeChange("ask")}
      />
    </div>
  ),
}));

// Thread Scope row — the sole empty-state scope surface (below the
// composer). Stubbed because its three popovers own their own store
// subscriptions / Tauri round-trips; these tests exercise the PANE's
// dispatch wiring, and the row's own rendering lives in
// ThreadScopeRow.test.tsx.
type ThreadScopeRowStubProps = {
  location:
    | {
        kind: "workspace";
        isHome: boolean;
        onSelectHomeWorkspace: (workspaceId: string) => void;
        onSelectProject: (projectPath: string) => void;
      }
    | { kind: "draft" };
  projectPath: string | null;
  // A workspace-bound pane always passes a FIXED scope — its workspace
  // has one checkout that every pane in it shares, so there is no
  // per-thread worktree choice to make here.
  scope:
    | { kind: "fixed"; branch: string | null }
    | { kind: "editable"; checkoutMode: "current" | "worktree" };
  disabled?: boolean;
};
let lastThreadScopeRowProps: ThreadScopeRowStubProps | null = null;
vi.mock("./pickers/ThreadScopeRow", () => ({
  // Class-string constants consumed by the Context Row's strip shell.
  SCOPE_STRIP: "scope-strip-stub",
  SCOPE_STRIP_INSET: "scope-strip-inset-stub",
  ThreadScopeRow: (props: ThreadScopeRowStubProps) => {
    lastThreadScopeRowProps = props;
    return (
      <button
        data-testid="thread-scope-row-stub"
        data-location-kind={props.location.kind}
        data-is-home={
          props.location.kind === "workspace" && props.location.isHome
            ? "true"
            : "false"
        }
        data-project-path={props.projectPath ?? ""}
        data-scope-kind={props.scope.kind}
        data-base-branch={
          props.scope.kind === "fixed" ? props.scope.branch ?? "" : ""
        }
      />
    );
  },
}));

// Status cluster — its own store subscriptions / Tauri round-trips are
// covered by WorkspaceStatusCluster.test.tsx; these tests only need to
// see WHERE it's rendered (Context Row vs. the empty-state scope row).
vi.mock("./WorkspaceStatusCluster", () => ({
  WorkspaceStatusCluster: () => (
    <div data-testid="workspace-status-cluster-stub" />
  ),
}));

vi.mock("@/stores/ui-store", () => ({
  useUIStore: Object.assign(
    vi.fn((selector: (s: unknown) => unknown) =>
      selector({ setShowNewWorkspaceDialog: setShowNewWorkspaceDialogMock }),
    ),
    {
      getState: () => ({
        setShowNewWorkspaceDialog: setShowNewWorkspaceDialogMock,
      }),
    },
  ),
}));

vi.mock("@/hooks/use-agent-chat-events", () => ({
  useAgentChatEvents: () => {},
}));

vi.mock("@/tauri/events", () => ({
  onAgentChatTurnCheckpoint: vi.fn().mockResolvedValue(() => {}),
  onAgentChatTurnCheckpointReverted: vi.fn().mockResolvedValue(() => {}),
  onAgentChatTurnCheckpointsInvalidated: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock("@/tauri/commands", () => ({
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  agentChatCreatePane: vi.fn().mockResolvedValue("pane-new"),
  // Provider-health probe (ProviderStatusNotice mount). Never resolves
  // so no banner state churns mid-test; banner tests drive the store
  // directly (see ProviderStatusNotice.test.tsx).
  agentChatProviderHealth: vi.fn(() => new Promise(() => {})),
  // The pane's mount-seed effect (design F) fetches the persisted
  // session row to restore picker config + resume cursor. Default to
  // `null` (no persisted row) so tests that don't seed a record fall
  // back to the provider default; the seed tests override per-case.
  agentChatGetSession: vi.fn().mockResolvedValue(null),
  agentChatInterruptTurn: vi.fn().mockResolvedValue(undefined),
  // The pane's mount-time hydrate effect calls this whenever it lands
  // with a truthy threadId — return an empty transcript so the effect
  // short-circuits without exercising the resume path in unit tests.
  agentChatListMessages: vi.fn().mockResolvedValue([]),
  // Cursor resume reads. `after` returns rows (`{id, payload}`); the
  // head probe guards against a cursor from a foreign id space.
  agentChatListMessagesAfter: vi.fn().mockResolvedValue([]),
  agentChatThreadHeadId: vi.fn().mockResolvedValue(null),
  // Liveness probe fired right after the transcript read on the
  // hydrate path. Default to `false` (no live turn) so hydrate keeps its
  // resume-path behavior unless a test opts into the live-run case.
  agentChatTurnActive: vi.fn().mockResolvedValue(false),
  // The D2 session-start marker effect looks the active thread up in the
  // persisted sessions list; default to empty so it clears to the plain
  // divider unless a test seeds a record.
  agentChatListSessions: vi.fn().mockResolvedValue([]),
  agentChatListTurnCheckpoints: vi.fn().mockResolvedValue([]),
  agentChatRevertTurnCheckpoint: vi.fn().mockResolvedValue([]),
  agentChatRespondToRequest: vi.fn().mockResolvedValue(undefined),
  agentChatCancelQueuedTurn: vi.fn().mockResolvedValue(true),
  agentChatSendTurn: vi.fn().mockResolvedValue(undefined),
  agentChatSendQueuedTurnNow: vi.fn().mockResolvedValue(undefined),
  agentChatSetModel: vi.fn().mockResolvedValue(undefined),
  agentChatSetFastMode: vi.fn().mockResolvedValue(undefined),
  agentChatSetPermissionMode: vi.fn().mockResolvedValue(undefined),
  agentChatStartSession: vi.fn().mockResolvedValue("thread-new"),
  agentChatStopSession: vi.fn().mockResolvedValue(undefined),
  // Picker handlers (design G) mirror every change into this DB-only
  // persist command, fire-and-forget. Default no-op.
  agentChatUpdateSessionConfig: vi.fn().mockResolvedValue(undefined),
  grepCountPattern: vi.fn().mockResolvedValue(0),
  // Thread Scope deferred-worktree path — AgentChatPane imports
  // `createDeferredWorktree` from materialize.ts, which pulls these
  // command bindings in transitively. Defaults match the shape the
  // deferred-submit tests assert on; per-test overrides as needed.
  applyPreset: vi.fn().mockResolvedValue(undefined),
  createEmptyWorkspace: vi.fn().mockResolvedValue("ws-empty"),
  // `createDeferredWorktree` calls the *Result variant. `cwd: null` keeps
  // the worktree cwd resolving through the store (`waitForWorkspaceCwd`),
  // matching the pre-fix path these tests assert on.
  createWorktreeWorkspaceResult: vi
    .fn()
    .mockResolvedValue({ workspaceId: "ws-new", cwd: null, adopted: false }),
  generateBranchName: vi.fn().mockResolvedValue("ai-named-branch"),
  generateRandomBranchName: vi.fn().mockResolvedValue("random-branch"),
  getHomeDir: vi.fn().mockResolvedValue("/home/user"),
  renameWorkspace: vi.fn().mockResolvedValue(undefined),
  // MCP warmup fired on the empty-state mount; no-op in tests.
  primeChatMcp: vi.fn().mockResolvedValue(undefined),
  // Host-scoped source-control preflight behind the composer's hosting
  // attach rows. Default to a fully usable checkout; the gating tests
  // override per-case.
  checkProviderAuth: vi.fn().mockResolvedValue({
    kind: "github",
    supported: true,
    installed: true,
    authenticated: true,
    username: "test",
  }),
  // Image staging (imported by the image-staging helper); only invoked
  // when a test stages an image, which these don't.
  stageChatImage: vi
    .fn()
    .mockResolvedValue({ path: "/staging/x.png", media_type: "image/png" }),
  discardStagedChatImage: vi.fn().mockResolvedValue(undefined),
  readChatImage: vi
    .fn()
    .mockResolvedValue({ bytes: new Uint8Array(), media_type: "image/png" }),
}));

const HOME_APP_STATE = {
  appState: {
    active_workspace_id: "ws-home",
    workspaces: [
      {
        workspace_id: "ws-home",
        // Home detection after the Stage B rework is path-based:
        // `project_root === homeDir`. `workspace_type` is irrelevant.
        workspace_type: "standard",
        project_root: "/home/user",
        cwd: "/home/user",
      },
    ],
  },
  homeDir: "/home/user",
};
const mockAppState: { appState: unknown; homeDir: string | null } =
  { ...HOME_APP_STATE };

vi.mock("@/stores/app-store", () => ({
  useAppStore: Object.assign(
    vi.fn((selector) => selector(mockAppState)),
    { getState: () => mockAppState },
  ),
  useHomeDir: () => "/home/user",
  findWorkspaceIdForPane: () => workspaceIdForPaneOverride,
  // Pure helper used inside the Home-target ProjectPicker onChange to
  // resolve the workspace to activate. Mirrors the real
  // groupWorkspacesByProject's signature.
  groupWorkspacesByProject: (
    workspaces: Array<{ project_root: string; workspace_id: string }>,
  ) =>
    Array.from(
      workspaces
        .reduce(
          (acc, ws) => {
            const path = ws.project_root;
            if (!acc.has(path)) {
              acc.set(path, { projectPath: path, workspaces: [] });
            }
            acc.get(path)!.workspaces.push(ws);
            return acc;
          },
          new Map<
            string,
            {
              projectPath: string;
              workspaces: Array<{ workspace_id: string }>;
            }
          >(),
        )
        .values(),
    ),
}));

vi.mock("@/stores/chat-draft-store", () => ({
  useChatDraftStore: Object.assign(
    vi.fn((selector) =>
      selector({
        draftsById: currentDraftsById,
        setActiveDraft: setActiveDraftMock,
      }),
    ),
    {
      getState: () => ({
        draftsById: currentDraftsById,
        setActiveDraft: setActiveDraftMock,
      }),
    },
  ),
}));

vi.mock("@/stores/agent-chat-store", () => {
  function makeSlice(messages: unknown[], overrides: SliceOverrides = {}) {
    return {
      messages,
      inputDraft: overrides.inputDraft ?? "",
      streaming: overrides.streaming ?? false,
      activeTurnId: null,
      model: overrides.model ?? null,
      permissionMode: overrides.permissionMode ?? "bypassPermissions",
      sessionLaunchMode:
        overrides.sessionLaunchMode ?? "bypassPermissions",
      resumeCursor: overrides.resumeCursor ?? null,
      mode: overrides.mode ?? "default",
      modePriorPermissionMode: overrides.modePriorPermissionMode ?? null,
      effort: overrides.effort ?? null,
      contextWindow: overrides.contextWindow ?? null,
      fastMode: overrides.fastMode ?? false,
      hasDebugActivity: overrides.hasDebugActivity ?? false,
      debugActivityResolved: overrides.debugActivityResolved ?? false,
      pendingRequestIds: [],
      lastPersistedEventId: overrides.lastPersistedEventId ?? null,
    };
  }
  function buildThreads() {
    // Merge the legacy `thread-x` (used by existing tests) with any
    // per-test `currentThreadsMap` entries so new tests can seed a
    // specific thread id like `draft-thread-42`. Per-thread slice
    // overrides come from `currentSliceOverrides` so a test can seed
    // mode / modePriorPermissionMode without touching the messages map.
    const threads: Record<string, unknown> = {
      "thread-x": makeSlice(
        currentMessages,
        currentSliceOverrides["thread-x"],
      ),
    };
    for (const [tid, msgs] of Object.entries(currentThreadsMap)) {
      threads[tid] = makeSlice(msgs, currentSliceOverrides[tid]);
    }
    return threads;
  }
  const mockStore = Object.assign(
    vi.fn((selector: (state: unknown) => unknown) => {
      const state = {
        threads: buildThreads(),
        ensureThread: vi.fn(),
        setInputDraft: setInputDraftMock,
        setModel: setModelMock,
        setPermissionMode: setPermissionModeMock,
        setSessionLaunchMode: vi.fn(),
        setEffort: setEffortMock,
        setContextWindow: setContextWindowMock,
        setFastMode: setFastModeMock,
        setResumeCursor: setResumeCursorMock,
        setMode: setModeMock,
        setModePriorPermissionMode: setModePriorMock,
        setHasDebugActivity: setHasDebugActivityMock,
        setDebugActivityResolved: setDebugActivityResolvedMock,
        migrateThreadId: vi.fn(),
        appendUserMessage: vi.fn(),
        removeUserMessageByNonce: vi.fn(),
        addStagedAttachment: vi.fn(),
        updateStagedAttachment: vi.fn(),
        removeStagedAttachment: vi.fn(),
        clearStagedAttachments: vi.fn(),
        markRequestResponding: vi.fn(),
        markRequestResolved: markRequestResolvedMock,
        applyEvent: vi.fn(),
      };
      return selector(state);
    }),
    {
      getState: () => ({
        threads: buildThreads(),
        applyEvent: applyEventMock,
        // Session bring-up seeds the agent-chat slice through these
        // after start_session resolves.
        ensureThread: vi.fn(),
        setPermissionMode: vi.fn(),
        setSessionLaunchMode: vi.fn(),
        setModel: setModelMock,
        setEffort: setEffortMock,
        setContextWindow: setContextWindowMock,
        setFastMode: setFastModeMock,
        setMode: setModeMock,
        // Thread Scope deferred-worktree submit seeds the NEW thread's
        // optimistic user bubble through getState().
        appendUserMessage: appendUserMessageMock,
        // Exposed on getState so the mount-time hydrate effect can
        // imperatively replace the slice (cold) or fold in a cursor
        // tail (warm).
        hydrateThread: hydrateThreadMock,
        applyPayloadsTail: applyPayloadsTailMock,
        invalidateThreadCursor: invalidateThreadCursorMock,
        evictColdThreads: vi.fn(),
      }),
    },
  );
  return {
    useAgentChatStore: mockStore,
    DEFAULT_THREAD_PERMISSION_MODE: "bypassPermissions",
    // The pane registers its thread so warm-slice eviction can't drop a
    // transcript that is on screen. Returns the unregister.
    registerMountedThread: () => registerMountedThreadMock(),
  };
});

import { AgentChatPane } from "./AgentChatPane";
import {
  agentChatGetSession,
  agentChatInterruptTurn,
  agentChatListMessages,
  agentChatListMessagesAfter,
  agentChatThreadHeadId,
  agentChatTurnActive,
  agentChatListSessions,
  agentChatRespondToRequest,
  agentChatSendTurn,
  agentChatSetFastMode,
  agentChatSetModel,
  agentChatSetPermissionMode,
  agentChatStartSession,
  agentChatStopSession,
  agentChatUpdateSessionConfig,
  grepCountPattern,
  checkProviderAuth,
  type AgentChatSessionRecord,
} from "@/tauri/commands";
import { _resetProviderAuthCache, NO_OPERATIONS } from "@/lib/provider-auth";
import { useProviderCapabilities } from "@/stores/provider-capabilities-store";
import type { ChatModelInfo } from "@/tauri/types";

const pane = {
  kind: "agent_chat" as const,
  pane_id: "pane-1",
  title: "Chat",
  thread_id: "thread-x",
  provider: "claude" as const,
  cwd: "/home/user",
};

describe("AgentChatPane empty-state branch", () => {
  beforeEach(() => {
    currentMessages = [];
    currentThreadsMap = {};
    currentDraftsById = {};
    workspaceIdForPaneOverride = "ws-home";
    vi.mocked(agentChatStartSession).mockClear();
  });

  it("renders ChatHomeLanding when messages.length === 0", () => {
    currentMessages = [];
    const { container } = render(<AgentChatPane pane={pane} />);
    expect(
      container.querySelector('[data-testid="home-landing"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="transcript"]'),
    ).toBeNull();
  });

  it("renders ChatTranscript + Composer when messages.length >= 1", () => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    const { container } = render(<AgentChatPane pane={pane} />);
    expect(
      container.querySelector('[data-testid="transcript"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="composer"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="home-landing"]'),
    ).toBeNull();
  });

  it("requests composer focus when a local first send swaps in the transcript", async () => {
    currentMessages = [];
    currentSliceOverrides = { "thread-x": { inputDraft: "hello" } };
    const view = render(<AgentChatPane pane={pane} />);

    fireEvent.click(
      view.container.querySelector('[data-testid="composer-submit"]')!,
    );
    await waitFor(() => {
      expect(
        view.container.querySelector('[data-testid="composer"]'),
      ).toHaveAttribute(
        "data-focus-on-mount",
        "true",
      );
    });

    currentMessages = [{ kind: "user_message", id: "m1" }];
    view.rerender(<AgentChatPane pane={pane} />);

    expect(
      view.container.querySelector('[data-testid="composer"]'),
    ).toHaveAttribute(
      "data-focus-on-mount",
      "true",
    );
  });
});

// ── Source-control preflight → composer gating ──
//
// The pane owns the probe; the composer only renders what it is handed.
// A CLI that is not installed used to arrive as `null` ("the auth
// question doesn't apply"), which the composer's gate read as "nothing
// known to be wrong" and left the hosting attach rows live onto a
// picker that could only error. It has to arrive as a plain `false`,
// with the missing-CLI nuance carried by its own flag.
describe("AgentChatPane source-control preflight", () => {
  beforeEach(() => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    currentThreadsMap = {};
    currentDraftsById = {};
    workspaceIdForPaneOverride = "ws-home";
    _resetProviderAuthCache();
  });

  it("reports a missing provider CLI as not authenticated", async () => {
    vi.mocked(checkProviderAuth).mockResolvedValueOnce({
      kind: "github",
      supported: true,
      installed: false,
      authenticated: false,
      operations: NO_OPERATIONS,
      username: null,
    });
    const { container } = render(<AgentChatPane pane={pane} />);
    await waitFor(() => {
      const composer = container.querySelector('[data-testid="composer"]');
      expect(composer?.getAttribute("data-provider-cli-installed")).toBe(
        "false",
      );
      expect(composer?.getAttribute("data-provider-authenticated")).toBe(
        "false",
      );
    });
  });

  it("passes a usable CLI through as authenticated", async () => {
    const { container } = render(<AgentChatPane pane={pane} />);
    await waitFor(() => {
      const composer = container.querySelector('[data-testid="composer"]');
      expect(composer?.getAttribute("data-provider-cli-installed")).toBe(
        "true",
      );
      expect(composer?.getAttribute("data-provider-authenticated")).toBe(
        "true",
      );
    });
  });
});

describe("AgentChatPane Continue-run chip (issue #154)", () => {
  beforeEach(async () => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    currentThreadsMap = {};
    currentDraftsById = {};
    currentSliceOverrides = {};
    workspaceIdForPaneOverride = "ws-home";
    const { agentChatSendTurn } = await import("@/tauri/commands");
    vi.mocked(agentChatSendTurn).mockClear().mockResolvedValue({
      turn_id: "turn-1",
      queued_id: null,
    } as never);
  });

  it("sends a plain 'Continue' turn with NO images (staged attachments left intact)", async () => {
    // Seed a non-empty composer draft so the test also proves the Continue
    // send does not depend on the draft (it sends the literal 'Continue').
    currentSliceOverrides = {
      "thread-x": { inputDraft: "half-typed next message" },
    };
    const { container } = render(<AgentChatPane pane={pane} />);
    const { agentChatSendTurn } = await import("@/tauri/commands");
    fireEvent.click(
      container.querySelector('[data-testid="composer-continue"]')!,
    );
    await waitFor(() => {
      expect(vi.mocked(agentChatSendTurn)).toHaveBeenCalledTimes(1);
    });
    const [, sendInput] = vi.mocked(agentChatSendTurn).mock.calls[0];
    expect(sendInput.thread_id).toBe("thread-x");
    expect(sendInput.text).toBe("Continue");
    // No images travel with a Continue send even if attachments were staged.
    expect(sendInput.images).toEqual([]);
  });
});

describe("AgentChatPane new-turn scroll contract (send anchor)", () => {
  const anchorNonce = (container: HTMLElement) =>
    container
      .querySelector('[data-testid="transcript"]')!
      .getAttribute("data-send-anchor-nonce");
  const anchorClientNonce = (container: HTMLElement) =>
    container
      .querySelector('[data-testid="transcript"]')!
      .getAttribute("data-send-anchor-client-nonce");

  beforeEach(async () => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    currentThreadsMap = {};
    currentDraftsById = {};
    currentSliceOverrides = { "thread-x": { inputDraft: "hello there" } };
    workspaceIdForPaneOverride = "ws-home";
    appendUserMessageMock.mockClear();
    setInputDraftMock.mockClear();
    const { agentChatCancelQueuedTurn, agentChatSendTurn } = await import(
      "@/tauri/commands"
    );
    vi.mocked(agentChatCancelQueuedTurn).mockClear().mockResolvedValue(true);
    vi.mocked(agentChatSendTurn).mockClear().mockResolvedValue({
      turn_id: "turn-1",
      queued_id: null,
    } as never);
  });

  it("anchors the exact bubble it appended, and reports it to the transcript", async () => {
    const { container } = render(<AgentChatPane pane={pane} />);
    expect(anchorNonce(container)).toBe("");

    fireEvent.click(container.querySelector('[data-testid="composer-submit"]')!);
    const { agentChatSendTurn } = await import("@/tauri/commands");
    await waitFor(() => {
      expect(vi.mocked(agentChatSendTurn)).toHaveBeenCalledTimes(1);
    });

    // One identity for the optimistic bubble, the anchor, and the turn the
    // backend was asked to run — so the transcript anchors the row the user
    // actually just sent rather than whatever happens to be last.
    const [, sendInput] = vi.mocked(agentChatSendTurn).mock.calls[0];
    await waitFor(() => {
      expect(anchorClientNonce(container)).toBe(sendInput.client_nonce);
    });
    expect(anchorClientNonce(container)).not.toBe("");
    expect(anchorNonce(container)).toBe("1");
  });

  it("gives the one-click Continue run the same contract", async () => {
    const { container } = render(<AgentChatPane pane={pane} />);
    fireEvent.click(
      container.querySelector('[data-testid="composer-continue"]')!,
    );
    const { agentChatSendTurn } = await import("@/tauri/commands");
    await waitFor(() => {
      expect(vi.mocked(agentChatSendTurn)).toHaveBeenCalledTimes(1);
    });
    const [, sendInput] = vi.mocked(agentChatSendTurn).mock.calls[0];
    await waitFor(() => {
      expect(anchorClientNonce(container)).toBe(sendInput.client_nonce);
    });
  });

  it("issues a fresh nonce per send so two identical prompts are two intents", async () => {
    const { agentChatSendTurn } = await import("@/tauri/commands");
    // The first send fails, which both clears the anchor and releases the
    // in-flight guard; the retry must read as a *new* navigation intent even
    // though the prompt text is byte-identical.
    vi.mocked(agentChatSendTurn)
      .mockRejectedValueOnce(new Error("boom") as never)
      .mockResolvedValue({ turn_id: "turn-1", queued_id: null } as never);

    const { container } = render(<AgentChatPane pane={pane} />);
    fireEvent.click(container.querySelector('[data-testid="composer-submit"]')!);
    await waitFor(() => expect(anchorNonce(container)).toBe(""));

    fireEvent.click(container.querySelector('[data-testid="composer-submit"]')!);
    await waitFor(() => expect(anchorNonce(container)).toBe("2"));
  });

  it("anchors a queued follow-up dispatched with 'send now'", async () => {
    // The bubble already exists (appended optimistically when it queued),
    // so its own nonce is the identity to anchor on.
    currentMessages = [
      { kind: "user_message", id: "m1", seq: 0, text: "first" },
      {
        kind: "user_message",
        id: "m2",
        seq: 1,
        text: "queued follow-up",
        clientNonce: "nonce-queued",
        queued: { queuedId: "q-1" },
      },
    ];
    const { container } = render(<AgentChatPane pane={pane} />);
    fireEvent.click(
      container.querySelector('[data-testid="send-queued-now"]')!,
    );
    await waitFor(() => {
      expect(anchorClientNonce(container)).toBe("nonce-queued");
    });
    const { agentChatSendQueuedTurnNow } = await import("@/tauri/commands");
    expect(vi.mocked(agentChatSendQueuedTurnNow)).toHaveBeenCalledWith(
      expect.anything(),
      "thread-x",
      "q-1",
    );
  });

  it("clears the anchor when a queued 'send now' dispatch fails", async () => {
    // There is no rollback path for a queued bubble — it just stays queued —
    // so the anchor has to be released explicitly or the list sits reserving
    // space for a turn that will never stream.
    currentMessages = [
      { kind: "user_message", id: "m1", seq: 0, text: "first" },
      {
        kind: "user_message",
        id: "m2",
        seq: 1,
        text: "queued follow-up",
        clientNonce: "nonce-queued",
        queued: { queuedId: "q-1" },
      },
    ];
    const { agentChatSendQueuedTurnNow } = await import("@/tauri/commands");
    vi.mocked(agentChatSendQueuedTurnNow).mockRejectedValueOnce(
      new Error("boom") as never,
    );

    const { container } = render(<AgentChatPane pane={pane} />);
    fireEvent.click(
      container.querySelector('[data-testid="send-queued-now"]')!,
    );
    await waitFor(() => expect(anchorClientNonce(container)).toBe(""));
  });

  it("restores a queued message only when the backend actually cancels it", async () => {
    currentSliceOverrides = {
      "thread-x": { inputDraft: "draft already in progress" },
    };
    const { container } = render(<AgentChatPane pane={pane} />);
    fireEvent.click(container.querySelector('[data-testid="cancel-queued"]')!);

    const { agentChatCancelQueuedTurn } = await import("@/tauri/commands");
    await waitFor(() => {
      expect(vi.mocked(agentChatCancelQueuedTurn)).toHaveBeenCalledWith(
        expect.anything(),
        "thread-x",
        "q-1",
      );
    });
    expect(setInputDraftMock).toHaveBeenCalledWith(
      "thread-x",
      "queued follow-up\ndraft already in progress",
    );
  });

  it("does not resurrect an already-dispatched queued message in the composer", async () => {
    const { agentChatCancelQueuedTurn } = await import("@/tauri/commands");
    vi.mocked(agentChatCancelQueuedTurn).mockResolvedValueOnce(false);
    const { container } = render(<AgentChatPane pane={pane} />);
    fireEvent.click(container.querySelector('[data-testid="cancel-queued"]')!);

    await waitFor(() => {
      expect(vi.mocked(agentChatCancelQueuedTurn)).toHaveBeenCalledWith(
        expect.anything(),
        "thread-x",
        "q-1",
      );
    });
    expect(setInputDraftMock).not.toHaveBeenCalled();
  });

  it("keeps the anchor when the turn settles", async () => {
    // The anchor outlives the turn: the reserved space collapsing at settle
    // is exactly the viewport yank the contract exists to prevent, so the
    // parked prompt (and the space under it) must not move when the run
    // finishes. The anchor is replaced by the next send and dropped on
    // rollback or thread switch instead.
    currentSliceOverrides = {
      "thread-x": { inputDraft: "hello there", streaming: true },
    };
    const { container, rerender } = render(<AgentChatPane pane={pane} />);
    fireEvent.click(container.querySelector('[data-testid="composer-submit"]')!);
    await waitFor(() => expect(anchorNonce(container)).toBe("1"));

    // The run finishes — nothing about the anchor changes.
    currentSliceOverrides = {
      "thread-x": { inputDraft: "hello there", streaming: false },
    };
    rerender(<AgentChatPane pane={pane} />);
    await waitFor(() =>
      expect(container.querySelector('[data-testid="transcript"]')).not.toBeNull(),
    );
    expect(anchorNonce(container)).toBe("1");
  });

  it("clears the anchor when the send fails, so no reserved space is stranded", async () => {
    const { agentChatSendTurn } = await import("@/tauri/commands");
    vi.mocked(agentChatSendTurn).mockRejectedValue(new Error("boom") as never);

    const { container } = render(<AgentChatPane pane={pane} />);
    fireEvent.click(container.querySelector('[data-testid="composer-submit"]')!);
    await waitFor(() => {
      expect(vi.mocked(agentChatSendTurn)).toHaveBeenCalledTimes(1);
    });
    // The optimistic bubble was rolled back; its reserved response space
    // has to go with it.
    await waitFor(() => expect(anchorClientNonce(container)).toBe(""));
    expect(anchorNonce(container)).toBe("");
  });
});

describe("AgentChatPane subagent drill-in (viewMode swap)", () => {
  const subagentMessage = {
    kind: "subagent_run",
    id: "run-1",
    seq: 1,
    turn_id: "turn-1",
    subagents: [
      {
        id: "sub-1",
        name: "Explore",
        model: "opus · xhigh",
        status: "running",
        items: [],
        toneIndex: 0,
      },
    ],
  };

  beforeEach(() => {
    currentMessages = [{ kind: "user_message", id: "m1", seq: 0 }, subagentMessage];
    currentThreadsMap = {};
    currentDraftsById = {};
    workspaceIdForPaneOverride = "ws-home";
    vi.mocked(agentChatListMessages).mockReset();
    vi.mocked(agentChatListMessages).mockResolvedValue([]);
  });

  it("swaps the transcript for the breadcrumb + read-only drill-in on enter, and Esc returns", () => {
    const { container } = render(<AgentChatPane pane={pane} />);

    // Orchestrator mode: the (mocked) transcript is shown, no breadcrumb.
    expect(container.querySelector('[data-testid="transcript"]')).not.toBeNull();

    // Enter the subagent via the real onEnterSubagent handler.
    fireEvent.click(container.querySelector('[data-testid="enter-subagent"]')!);

    // Drill-in mode: transcript gone, breadcrumb + read-only banner shown.
    expect(container.querySelector('[data-testid="transcript"]')).toBeNull();
    expect(container.textContent).toContain("Orchestrator");
    expect(container.textContent).toContain("Read-only view of the");
    // The drill-in shows the subagent name in both the banner and breadcrumb.
    expect(container.textContent).toContain("Explore");

    // Esc returns to the orchestrator transcript.
    fireEvent.keyDown(window, { key: "Escape" });
    expect(container.querySelector('[data-testid="transcript"]')).not.toBeNull();
  });

  it("keeps the parent-bound composer mounted while entered", () => {
    const { container } = render(<AgentChatPane pane={pane} />);
    expect(container.querySelector('[data-testid="composer"]')).not.toBeNull();
    fireEvent.click(container.querySelector('[data-testid="enter-subagent"]')!);
    // Drill-in swaps the transcript body but the composer stays parent-bound
    // (design: "steering goes to the orchestrator").
    expect(container.querySelector('[data-testid="transcript"]')).toBeNull();
    expect(container.querySelector('[data-testid="composer"]')).not.toBeNull();
  });

  it("hides the composer's running-subagents strip while drilled into a subagent, and shows it again on Esc", () => {
    const { container } = render(<AgentChatPane pane={pane} />);
    // Orchestrator mode: one live subagent — the strip is up, and it is
    // mounted inside the composer rather than docked above it.
    expect(
      container.querySelector(
        '[data-testid="composer-top-strip"] [data-testid="subagent-activity-bar"]',
      ),
    ).not.toBeNull();

    fireEvent.click(container.querySelector('[data-testid="enter-subagent"]')!);
    // Design: the strip only shows in the conversation view.
    expect(
      container.querySelector('[data-testid="subagent-activity-bar"]'),
    ).toBeNull();

    fireEvent.keyDown(window, { key: "Escape" });
    expect(
      container.querySelector('[data-testid="subagent-activity-bar"]'),
    ).not.toBeNull();
  });
});

describe("AgentChatPane hydrate-on-mount (cursor resume)", () => {
  // Regression base: the user submits a turn, switches workspaces (the
  // inactive workspace's pane tree unmounts entirely per
  // workspace-main.tsx), the live event broadcaster drops events with no
  // listener attached, and the user comes back to a chat that shows only
  // the optimistic user message — even though the agent ran and the
  // backend persisted the full transcript to SQLite.
  //
  // Phase 3 resumes that transcript BY CURSOR: a warm slice asks for the
  // rows after its `lastPersistedEventId`, a cold one reads the thread
  // from the start. The old rendered-message-count guard is gone — it
  // was not a durable position and was wrong in both directions.
  const userPayload = JSON.stringify({
    type: "user_message",
    thread_id: "thread-x",
    text: "hello",
  });
  // Persisted assistant reply, shaped per the reducer's
  // `item_completed` / `assistant_text` arms.
  const assistantPayload = JSON.stringify({
    type: "item_completed",
    thread_id: "thread-x",
    turn_id: "turn-1",
    item: { kind: "assistant_text", text: "hi back" },
  });
  const rows = [
    { id: 11, payload: userPayload },
    { id: 12, payload: assistantPayload },
  ];

  beforeEach(() => {
    currentMessages = [];
    currentThreadsMap = {};
    currentDraftsById = {};
    currentSliceOverrides = {};
    workspaceIdForPaneOverride = "ws-home";
    vi.mocked(agentChatListMessagesAfter).mockReset();
    vi.mocked(agentChatListMessagesAfter).mockResolvedValue([]);
    vi.mocked(agentChatThreadHeadId).mockReset();
    vi.mocked(agentChatThreadHeadId).mockResolvedValue(null);
    vi.mocked(agentChatTurnActive).mockReset();
    vi.mocked(agentChatTurnActive).mockResolvedValue(false);
    hydrateThreadMock.mockClear();
    applyPayloadsTailMock.mockClear();
    invalidateThreadCursorMock.mockClear();
  });

  it("reads from the start of the thread when the slice has no cursor", async () => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    render(<AgentChatPane pane={pane} />);
    await waitFor(() => {
      expect(vi.mocked(agentChatListMessagesAfter)).toHaveBeenCalledWith(
        "thread-x",
        null,
      );
    });
    // No cursor means no head probe — there is nothing to validate.
    expect(vi.mocked(agentChatThreadHeadId)).not.toHaveBeenCalled();
  });

  it("hydrates the slice from a cold read", async () => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    vi.mocked(agentChatListMessagesAfter).mockResolvedValue(rows);
    render(<AgentChatPane pane={pane} />);
    await waitFor(() => {
      expect(hydrateThreadMock).toHaveBeenCalledWith("thread-x", rows, {
        runLive: false,
        provider: "claude",
      });
    });
    // Every completed cold pass uses the direct replay path. Component
    // lifecycle remounts (notably React StrictMode on slower CI runners)
    // may start another pass, so exact invocation count belongs to the
    // cursor-hydrate unit suite rather than this wiring test.
    expect(hydrateThreadMock).toHaveBeenCalled();
    for (const call of hydrateThreadMock.mock.calls) {
      expect(call).toEqual([
        "thread-x",
        rows,
        { runLive: false, provider: "claude" },
      ]);
    }
    expect(applyPayloadsTailMock).not.toHaveBeenCalled();
  });

  it("hydrates with runLive:true when the backend reports the turn is still active", async () => {
    // The workspace-switch false-positive: a healthy live run whose pane
    // remounted. The persisted tail has no terminal event after the last
    // user turn, but the backend confirms the turn is in flight, so
    // hydrate must pass runLive so the streaming marker shows instead of
    // the Run-interrupted divider.
    currentMessages = [{ kind: "user_message", id: "m1" }];
    vi.mocked(agentChatListMessagesAfter).mockResolvedValue(rows);
    vi.mocked(agentChatTurnActive).mockResolvedValue(true);
    render(<AgentChatPane pane={pane} />);
    await waitFor(() => {
      expect(hydrateThreadMock).toHaveBeenCalledWith("thread-x", rows, {
        runLive: true,
        provider: "claude",
      });
    });
  });

  it("falls back to interrupted (runLive:false) when the liveness probe rejects", async () => {
    // A backend without the command, or a transient failure, must degrade
    // to today's heuristic behavior rather than blocking hydrate.
    currentMessages = [{ kind: "user_message", id: "m1" }];
    vi.mocked(agentChatListMessagesAfter).mockResolvedValue(rows);
    vi.mocked(agentChatTurnActive).mockRejectedValue(
      new Error("command unavailable"),
    );
    render(<AgentChatPane pane={pane} />);
    await waitFor(() => {
      expect(hydrateThreadMock).toHaveBeenCalledWith("thread-x", rows, {
        runLive: false,
        provider: "claude",
      });
    });
  });

  it("asks only for the tail when the slice carries a cursor", async () => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    currentSliceOverrides = { "thread-x": { lastPersistedEventId: 10 } };
    vi.mocked(agentChatThreadHeadId).mockResolvedValue(12);
    vi.mocked(agentChatListMessagesAfter).mockResolvedValue(rows);
    render(<AgentChatPane pane={pane} />);
    await waitFor(() => {
      expect(applyPayloadsTailMock).toHaveBeenCalledWith("thread-x", rows, {
        runLive: false,
        provider: "claude",
      });
    });
    expect(vi.mocked(agentChatListMessagesAfter)).toHaveBeenCalledWith(
      "thread-x",
      10,
    );
    // The transcript prefix is never rebuilt on the warm path.
    expect(hydrateThreadMock).not.toHaveBeenCalled();
  });

  it("does no work at all when a warm thread has not moved", async () => {
    // The exit gate: revisiting an unchanged warm thread costs one empty
    // tail read and zero reductions.
    currentMessages = [{ kind: "user_message", id: "m1" }];
    currentSliceOverrides = { "thread-x": { lastPersistedEventId: 12 } };
    vi.mocked(agentChatThreadHeadId).mockResolvedValue(12);
    vi.mocked(agentChatListMessagesAfter).mockResolvedValue([]);
    render(<AgentChatPane pane={pane} />);
    await waitFor(() => {
      expect(vi.mocked(agentChatListMessagesAfter)).toHaveBeenCalledWith(
        "thread-x",
        12,
      );
    });
    expect(applyPayloadsTailMock).not.toHaveBeenCalled();
    expect(hydrateThreadMock).not.toHaveBeenCalled();
    // Not even the liveness probe — there is nothing to interpret.
    expect(vi.mocked(agentChatTurnActive)).not.toHaveBeenCalled();
  });

  it("cold-hydrates when the cursor sits above the thread's head", async () => {
    // A cursor inherited from a merged / deleted thread: its rows were
    // re-homed under another thread_id keeping their original ids, so the
    // tail read would look empty forever.
    currentMessages = [{ kind: "user_message", id: "m1" }];
    currentSliceOverrides = { "thread-x": { lastPersistedEventId: 900 } };
    vi.mocked(agentChatThreadHeadId).mockResolvedValue(12);
    vi.mocked(agentChatListMessagesAfter).mockImplementation(
      async (_thread: string, afterId: number | null) =>
        afterId === null ? rows : [],
    );
    render(<AgentChatPane pane={pane} />);
    await waitFor(() => {
      expect(hydrateThreadMock).toHaveBeenCalledWith("thread-x", rows, {
        runLive: false,
        provider: "claude",
      });
    });
    expect(applyPayloadsTailMock).not.toHaveBeenCalled();
  });

  it("swallows read failures and forgets the cursor so the next visit rebuilds", async () => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    currentSliceOverrides = { "thread-x": { lastPersistedEventId: 10 } };
    vi.mocked(agentChatListMessagesAfter).mockRejectedValue(
      new Error("simulated SQLite read failure"),
    );
    expect(() => render(<AgentChatPane pane={pane} />)).not.toThrow();
    await waitFor(() => {
      expect(invalidateThreadCursorMock).toHaveBeenCalledWith("thread-x");
    });
    expect(hydrateThreadMock).not.toHaveBeenCalled();
    expect(applyPayloadsTailMock).not.toHaveBeenCalled();
  });
});

describe("AgentChatPane session-start marker wiring (D2)", () => {
  // The pane resolves the active thread's `created_at` from the persisted
  // sessions list and hands it to ChatTranscript as `sessionStartedAt`;
  // the transcript stub echoes it into `data-session-started-at`.
  function makeSessionRecord(overrides: {
    thread_id: string;
    created_at: string;
  }): AgentChatSessionRecord {
    return {
      thread_id: overrides.thread_id,
      sdk_session_id: "sdk-1",
      workspace_id: "ws-home",
      cwd: "/home/user",
      provider: "claude",
      title: "Chat",
      created_at: overrides.created_at,
      last_active_at: overrides.created_at,
      model: null,
      effort: null,
      context_window: null,
      permission_mode: null,
    };
  }

  beforeEach(() => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    currentThreadsMap = {};
    currentDraftsById = {};
    workspaceIdForPaneOverride = "ws-home";
    // Reset the mount-time hydrate mock a sibling block left rejecting,
    // so its caught-and-logged failure doesn't spam this block's stderr.
    vi.mocked(agentChatListMessages).mockReset();
    vi.mocked(agentChatListMessages).mockResolvedValue([]);
    vi.mocked(agentChatListSessions).mockReset();
    vi.mocked(agentChatListSessions).mockResolvedValue([]);
  });

  it("passes the matching session's parsed created_at through to the transcript", async () => {
    const createdAt = "2026-07-03T14:52:00Z";
    vi.mocked(agentChatListSessions).mockResolvedValue([
      makeSessionRecord({ thread_id: "thread-x", created_at: createdAt }),
    ]);
    const { container } = render(<AgentChatPane pane={pane} />);
    await waitFor(() => {
      expect(vi.mocked(agentChatListSessions)).toHaveBeenCalledWith(
        "ws-home",
        "/home/user",
      );
    });
    await waitFor(() => {
      const transcript = container.querySelector('[data-testid="transcript"]');
      expect(transcript?.getAttribute("data-session-started-at")).toBe(
        String(Date.parse(createdAt)),
      );
    });
  });

  it("falls back to the plain divider (no timestamp) when no record matches the thread", async () => {
    vi.mocked(agentChatListSessions).mockResolvedValue([
      makeSessionRecord({
        thread_id: "some-other-thread",
        created_at: "2026-07-03T14:52:00Z",
      }),
    ]);
    const { container } = render(<AgentChatPane pane={pane} />);
    await waitFor(() => {
      expect(vi.mocked(agentChatListSessions)).toHaveBeenCalled();
    });
    // No await-able change to observe, so assert the steady state: the
    // stub kept its empty marker attribute.
    const transcript = container.querySelector('[data-testid="transcript"]');
    expect(transcript?.getAttribute("data-session-started-at")).toBe("");
  });

  it("ignores an unparseable created_at and renders the plain divider", async () => {
    vi.mocked(agentChatListSessions).mockResolvedValue([
      makeSessionRecord({ thread_id: "thread-x", created_at: "not-a-date" }),
    ]);
    const { container } = render(<AgentChatPane pane={pane} />);
    await waitFor(() => {
      expect(vi.mocked(agentChatListSessions)).toHaveBeenCalled();
    });
    const transcript = container.querySelector('[data-testid="transcript"]');
    expect(transcript?.getAttribute("data-session-started-at")).toBe("");
  });
});

describe("AgentChatPane Stage C race fix", () => {
  const paneNoThread = {
    kind: "agent_chat" as const,
    pane_id: "pane-new",
    title: "Chat",
    thread_id: null,
    provider: "claude" as const,
    cwd: "/home/user",
  };

  beforeEach(() => {
    currentMessages = [];
    currentThreadsMap = {};
    currentDraftsById = {};
    workspaceIdForPaneOverride = "ws-home";
    vi.mocked(agentChatStartSession).mockClear();
  });

  it("adopts a promoted draft's thread_id when pane.thread_id is null, and does NOT start a fresh session", () => {
    // Simulate the race window: pane created but start_session's
    // state emit hasn't landed yet, so `pane.thread_id` is still
    // null. A promoted draft claims this workspace and carries the
    // pre-minted thread id that materialize already seeded.
    currentDraftsById = {
      "draft-1": {
        draftId: "draft-1",
        threadId: "draft-thread-42",
        // paneId must match `paneNoThread.pane_id` — the selector is
        // pane-scoped (not just workspace-scoped) so a fresh second
        // pane in the same workspace doesn't accidentally adopt the
        // first pane's draft.
        promotedTo: { workspaceId: "ws-home", paneId: "pane-new" },
      },
    };
    currentThreadsMap = {
      "draft-thread-42": [{ kind: "user_message", id: "m1", text: "hello" }],
    };

    const { container } = render(<AgentChatPane pane={paneNoThread} />);

    // Transcript renders the seeded slice (one message), not the
    // empty default.
    const transcript = container.querySelector(
      '[data-testid="transcript"]',
    ) as HTMLElement | null;
    expect(transcript).not.toBeNull();
    expect(transcript!.getAttribute("data-message-count")).toBe("1");
    // Critical: no duplicate session started.
    expect(agentChatStartSession).not.toHaveBeenCalled();
  });

  it("requests focus for the composer mounted from a promoted draft", () => {
    currentDraftsById = {
      "draft-1": {
        draftId: "draft-1",
        threadId: "draft-thread-42",
        promotedTo: { workspaceId: "ws-home", paneId: "pane-new" },
      },
    };
    currentThreadsMap = {
      "draft-thread-42": [{ kind: "user_message", id: "m1", text: "hello" }],
    };

    const { container } = render(<AgentChatPane pane={paneNoThread} />);

    expect(container.querySelector('[data-testid="composer"]')).toHaveAttribute(
      "data-focus-on-mount",
      "true",
    );
  });

  it("starts a fresh session when pane.thread_id is null AND no promoted draft claims this workspace", () => {
    // No drafts → the pane is a true blank slate, so the existing
    // mint-and-start branch should still fire.
    currentDraftsById = {};
    render(<AgentChatPane pane={paneNoThread} />);
    expect(agentChatStartSession).toHaveBeenCalledTimes(1);
  });

  it("does not start a fresh session when pane.thread_id is already populated", () => {
    const paneWithThread = { ...paneNoThread, thread_id: "thread-x" };
    render(<AgentChatPane pane={paneWithThread} />);
    expect(agentChatStartSession).not.toHaveBeenCalled();
  });

  it("syncs local threadId state when pane.thread_id transitions from null to set after mount", () => {
    // Mount with thread_id null and no promoted draft → fresh session
    // branch fires.
    currentDraftsById = {};
    const { rerender, container } = render(
      <AgentChatPane pane={paneNoThread} />,
    );
    expect(agentChatStartSession).toHaveBeenCalledTimes(1);

    // Rerender with thread_id populated (simulating the delayed
    // state-emit from Rust landing after mount). The prop-sync effect
    // should update local state; the existing mint-branch's
    // `startAttempted` ref prevents a second call.
    currentThreadsMap = {
      "thread-late-arrival": [{ kind: "user_message", id: "m1" }],
    };
    rerender(
      <AgentChatPane pane={{ ...paneNoThread, thread_id: "thread-late-arrival" }} />,
    );
    // No additional session start.
    expect(agentChatStartSession).toHaveBeenCalledTimes(1);
    // Transcript now reflects the new slice's messages.
    const transcript = container.querySelector(
      '[data-testid="transcript"]',
    ) as HTMLElement | null;
    expect(transcript).not.toBeNull();
    expect(transcript!.getAttribute("data-message-count")).toBe("1");
  });
});

describe("AgentChatPane Thread Scope — the pane's scope is fixed, not chosen", () => {
  // A chat pane is bound to a REAL workspace, and that workspace owns
  // exactly one project root and one checkout, shared by every tab and
  // pane inside it. Nothing about a thread's scope is choosable here, so
  // the strip below the composer is the read-only Context Row from the
  // first render — the interactive `ThreadScopeRow` belongs to
  // `DraftChatSurface` alone.
  //
  // Both retired controls were app-wide relocations wearing per-thread
  // clothing: "New worktree" created a SECOND workspace off the parent
  // repo mid-send and moved the user into it (re-asking on every extra
  // chat tab, including tabs of an already worktree-backed workspace),
  // and the location picker activated another project's workspace,
  // abandoning whatever had been typed in this one.

  /** The below-composer strip, or null when none renders. */
  function strip(container: HTMLElement) {
    return container.querySelector('[data-testid="below-composer"] .scope-strip-stub');
  }

  beforeEach(() => {
    currentMessages = [];
    currentThreadsMap = {};
    currentSliceOverrides = {};
    currentDraftsById = {};
    workspaceIdForPaneOverride = "ws-home";
    setShowNewWorkspaceDialogMock.mockClear();
    setActiveDraftMock.mockClear();
    lastThreadScopeRowProps = null;
    Object.assign(mockAppState, HOME_APP_STATE);
  });

  function seedProjectWorkspace(
    overrides: Record<string, unknown> = {},
  ) {
    mockAppState.appState = {
      active_workspace_id: "ws-foo",
      workspaces: [
        {
          workspace_id: "ws-foo",
          workspace_type: "standard",
          project_root: "/projects/foo",
          cwd: "/projects/foo",
          ...overrides,
        },
      ],
    };
    workspaceIdForPaneOverride = "ws-foo";
    return {
      ...pane,
      pane_id: "pane-foo",
      thread_id: "thread-x",
      cwd: (overrides.cwd as string) ?? "/projects/foo",
    };
  }

  it("renders NO interactive scope row on an empty thread — only the read-only Context Row", () => {
    const projectPane = seedProjectWorkspace({ git_branch: "feat/login" });
    const { container } = render(<AgentChatPane pane={projectPane} />);
    expect(
      container.querySelector('[data-testid="thread-scope-row-stub"]'),
    ).toBeNull();
    expect(lastThreadScopeRowProps).toBeNull();
    const scoped = within(container);
    expect(scoped.getByText("foo")).toBeInTheDocument();
    expect(scoped.getByText("feat/login")).toBeInTheDocument();
    // Zone 1 stays empty — scope lives below the composer.
    const zone1 = container.querySelector('[data-testid="zone1"]');
    expect(zone1!.childElementCount).toBe(0);
    const below = container.querySelector('[data-testid="below-composer"]');
    expect(
      below!.querySelector('[data-testid="workspace-status-cluster-stub"]'),
    ).not.toBeNull();
  });

  it("keeps the SAME strip once the conversation has messages (scope never appears to change on first send)", () => {
    const projectPane = seedProjectWorkspace({ git_branch: "feat/login" });
    const empty = render(<AgentChatPane pane={projectPane} />);
    const beforeSend = strip(empty.container)!.innerHTML;
    cleanup();
    currentMessages = [{ kind: "user_message", id: "m1" }];
    const withMessages = render(<AgentChatPane pane={projectPane} />);
    expect(strip(withMessages.container)!.innerHTML).toBe(beforeSend);
  });

  it("a worktree-backed workspace's extra chat tab gets the same read-only strip (the reported bug)", () => {
    // `project_root` is the PARENT repo, `cwd` the linked worktree —
    // the exact shape that used to re-offer "New worktree" per tab.
    const wtPane = seedProjectWorkspace({
      cwd: "/worktrees/foo/fix-login",
      git_branch: "fix-login",
    });
    const { container } = render(<AgentChatPane pane={wtPane} />);
    expect(
      container.querySelector('[data-testid="thread-scope-row-stub"]'),
    ).toBeNull();
    const scoped = within(container);
    expect(scoped.getByText("fix-login")).toBeInTheDocument();
  });

  it("home-rooted pane shows Home with no branch and no project picker", () => {
    Object.assign(mockAppState, HOME_APP_STATE);
    workspaceIdForPaneOverride = "ws-home";
    const { container } = render(<AgentChatPane pane={pane} />);
    const scoped = within(container);
    expect(scoped.getByText("Home")).toBeInTheDocument();
    expect(scoped.queryByText("·")).not.toBeInTheDocument();
    expect(
      container.querySelector('[data-testid="thread-scope-row-stub"]'),
    ).toBeNull();
  });

  it("first send stays in THIS pane's thread — no worktree, no workspace, no relocation", async () => {
    const projectPane = seedProjectWorkspace({
      cwd: "/worktrees/foo/fix-login",
      git_branch: "fix-login",
    });
    currentSliceOverrides = {
      "thread-x": { inputDraft: "fix the login bug" },
    };
    const {
      activateWorkspace,
      agentChatSendTurn,
      createWorktreeWorkspaceResult,
      generateBranchName,
    } = await import("@/tauri/commands");
    vi.mocked(activateWorkspace).mockClear();
    vi.mocked(createWorktreeWorkspaceResult).mockClear();
    vi.mocked(generateBranchName).mockClear();
    vi.mocked(agentChatSendTurn).mockClear().mockResolvedValue({
      turn_id: "turn-1",
      queued_id: null,
    } as never);
    const { container } = render(<AgentChatPane pane={projectPane} />);
    fireEvent.click(
      container.querySelector('[data-testid="composer-submit"]')!,
    );
    await waitFor(() => {
      expect(vi.mocked(agentChatSendTurn)).toHaveBeenCalledTimes(1);
    });
    const [, sendInput] = vi.mocked(agentChatSendTurn).mock.calls[0];
    expect(sendInput.thread_id).toBe("thread-x");
    // The invariant is "no worktree, no second workspace, no relocation".
    // `generateBranchName` NOT being called used to stand in for that, but
    // it no longer can: the first send legitimately runs the namer to
    // TITLE this workspace (see the auto-naming test below), it just never
    // cuts a branch off it.
    expect(vi.mocked(createWorktreeWorkspaceResult)).not.toHaveBeenCalled();
    expect(vi.mocked(activateWorkspace)).not.toHaveBeenCalled();
  });

  it("names the workspace off the first message, without blocking the send", async () => {
    // The reported asymmetry: a worktree first send got a real name for
    // free (the backend overwrites the title with the new branch), while
    // a pane sending into its own checkout cuts no branch and so kept the
    // backend default forever.
    const projectPane = seedProjectWorkspace({ title: "Workspace 58" });
    currentSliceOverrides = {
      "thread-x": { inputDraft: "fix the login bug" },
    };
    const { agentChatSendTurn, generateBranchName, renameWorkspace } =
      await import("@/tauri/commands");
    vi.mocked(generateBranchName).mockClear();
    vi.mocked(renameWorkspace).mockClear();
    vi.mocked(agentChatSendTurn).mockClear().mockResolvedValue({
      turn_id: "turn-1",
      queued_id: null,
    } as never);
    const { container } = render(<AgentChatPane pane={projectPane} />);
    fireEvent.click(
      container.querySelector('[data-testid="composer-submit"]')!,
    );
    // The send lands first — naming is fire-and-forget behind it, so a
    // slow `claude --print` can never delay the turn.
    await waitFor(() => {
      expect(vi.mocked(agentChatSendTurn)).toHaveBeenCalledTimes(1);
    });
    expect(vi.mocked(generateBranchName)).toHaveBeenCalledWith(
      "fix the login bug",
      "/projects/foo",
    );
    await waitFor(() => {
      expect(vi.mocked(renameWorkspace)).toHaveBeenCalledWith(
        "ws-foo",
        "ai-named-branch",
      );
    });
  });

  it("names once when two Enter presses land in the same tick", async () => {
    // `messages.length === 0` (the gate that selects the naming handler)
    // only flips once the optimistic bubble lands, so both presses see an
    // empty thread. Only the send-in-flight ref rules the second one out,
    // and it has to do so BEFORE the namer runs — two `claude --print`
    // calls would otherwise race two different titles into the workspace,
    // the second landing before the first reached the store.
    const projectPane = seedProjectWorkspace({ title: "Workspace 58" });
    currentSliceOverrides = {
      "thread-x": { inputDraft: "fix the login bug" },
    };
    const { agentChatSendTurn, generateBranchName } = await import(
      "@/tauri/commands"
    );
    vi.mocked(generateBranchName).mockClear();
    vi.mocked(agentChatSendTurn).mockClear().mockResolvedValue({
      turn_id: "turn-1",
      queued_id: null,
    } as never);
    const { container } = render(<AgentChatPane pane={projectPane} />);
    const submit = container.querySelector('[data-testid="composer-submit"]')!;
    fireEvent.click(submit);
    fireEvent.click(submit);
    await waitFor(() => {
      expect(vi.mocked(agentChatSendTurn)).toHaveBeenCalledTimes(1);
    });
    expect(vi.mocked(generateBranchName)).toHaveBeenCalledTimes(1);
  });

  it("does not clobber a title the user already chose", async () => {
    const projectPane = seedProjectWorkspace({ title: "Payments rewrite" });
    currentSliceOverrides = {
      "thread-x": { inputDraft: "fix the login bug" },
    };
    const { agentChatSendTurn, renameWorkspace } = await import(
      "@/tauri/commands"
    );
    vi.mocked(renameWorkspace).mockClear();
    vi.mocked(agentChatSendTurn).mockClear().mockResolvedValue({
      turn_id: "turn-1",
      queued_id: null,
    } as never);
    const { container } = render(<AgentChatPane pane={projectPane} />);
    fireEvent.click(
      container.querySelector('[data-testid="composer-submit"]')!,
    );
    await waitFor(() => {
      expect(vi.mocked(agentChatSendTurn)).toHaveBeenCalledTimes(1);
    });
    // The guard is re-read AFTER the namer resolves, so drain the
    // fire-and-forget chain before asserting it declined to rename.
    await act(async () => {
      for (let i = 0; i < 10; i++) await Promise.resolve();
    });
    expect(vi.mocked(renameWorkspace)).not.toHaveBeenCalled();
  });

  it("does not name a home-rooted pane from its first message", async () => {
    // Home workspaces are not project checkouts — `generateBranchName`
    // has no repo to name a branch against or deconflict within.
    currentSliceOverrides = {
      "thread-x": { inputDraft: "fix the login bug" },
    };
    const { agentChatSendTurn, generateBranchName } = await import(
      "@/tauri/commands"
    );
    vi.mocked(generateBranchName).mockClear();
    vi.mocked(agentChatSendTurn).mockClear().mockResolvedValue({
      turn_id: "turn-1",
      queued_id: null,
    } as never);
    const homePane = { ...pane, pane_id: "pane-home", thread_id: "thread-x" };
    const { container } = render(<AgentChatPane pane={homePane} />);
    fireEvent.click(
      container.querySelector('[data-testid="composer-submit"]')!,
    );
    await waitFor(() => {
      expect(vi.mocked(agentChatSendTurn)).toHaveBeenCalledTimes(1);
    });
    expect(vi.mocked(generateBranchName)).not.toHaveBeenCalled();
  });

  it("renders no strip at all when appState is null (early-boot fallback)", () => {
    mockAppState.appState = null;
    workspaceIdForPaneOverride = null;
    const { container } = render(<AgentChatPane pane={pane} />);
    expect(container.querySelector('[data-testid="composer"]')).not.toBeNull();
    expect(strip(container)).toBeNull();
  });

  it("labels the strip from project_root, not cwd, when they differ", () => {
    const featPane = seedProjectWorkspace({ cwd: "/projects/foo-feat-x" });
    const { container } = render(<AgentChatPane pane={featPane} />);
    expect(strip(container)!.querySelector("span[title]")!.getAttribute("title")).toBe(
      "/projects/foo",
    );
  });

  it("falls back to active_workspace_id's project_root when workspaceIdForPane is null", () => {
    const projectPane = seedProjectWorkspace();
    workspaceIdForPaneOverride = null;
    const { container } = render(<AgentChatPane pane={projectPane} />);
    expect(within(container).getByText("foo")).toBeInTheDocument();
  });

  it("project_root null on the workspace falls back to cwd", () => {
    mockAppState.appState = {
      active_workspace_id: "ws-adhoc",
      workspaces: [
        {
          workspace_id: "ws-adhoc",
          workspace_type: "standard",
          project_root: null,
          cwd: "/tmp/adhoc",
        },
      ],
    };
    workspaceIdForPaneOverride = "ws-adhoc";
    const adhocPane = {
      ...pane,
      pane_id: "pane-adhoc",
      thread_id: "thread-x",
      cwd: "/tmp/adhoc",
    };
    const { container } = render(<AgentChatPane pane={adhocPane} />);
    expect(within(container).getByText("adhoc")).toBeInTheDocument();
  });
});

// ── Default-model seed effect ─────────────────────────────────────────
//
// Regression coverage for the bug where the ReasoningPicker disappeared
// after app restart / resume / any other path that left the slice with
// `model: null` under a pre-existing thread_id. The mount effect at the
// top of AgentChatPane short-circuits when threadId is set, so without
// a separate seed effect the slice's model would stay null and
// ReasoningPicker (`if (!model) return null`) would render nothing.

describe("AgentChatPane mount-seed effect (design F)", () => {
  function makeRecord(
    overrides: Partial<AgentChatSessionRecord> = {},
  ): AgentChatSessionRecord {
    return {
      thread_id: "thread-x",
      sdk_session_id: null,
      workspace_id: "ws-home",
      cwd: "/home/user",
      provider: "claude",
      title: "Chat",
      created_at: "2026-07-03T14:52:00Z",
      last_active_at: "2026-07-03T14:52:00Z",
      model: null,
      effort: null,
      context_window: null,
      permission_mode: null,
      ...overrides,
    };
  }

  beforeEach(() => {
    currentMessages = [];
    currentThreadsMap = {};
    currentDraftsById = {};
    currentSliceOverrides = {};
    workspaceIdForPaneOverride = "ws-home";
    setModelMock.mockClear();
    setEffortMock.mockClear();
    setContextWindowMock.mockClear();
    setFastModeMock.mockClear();
    setResumeCursorMock.mockClear();
    setPermissionModeMock.mockClear();
    vi.mocked(agentChatStartSession).mockClear();
    vi.mocked(agentChatGetSession).mockReset().mockResolvedValue(null);
    mockAppState.appState = HOME_APP_STATE.appState;
  });
  afterEach(() => {
    // Without an explicit unmount the prior test's pane stays in the
    // DOM and its async session-start `.then(...)` callback fires its
    // setStoreModel call into the next test's accumulated mock.calls
    // — exactly the pollution that hid this bug initially. cleanup()
    // unmounts every render() from this test file.
    cleanup();
  });

  it("falls back to the provider default when no persisted row exists", async () => {
    // Pane already has a thread_id (existing pane reopened, or app
    // restart hydrated the pane snapshot but the in-memory store is
    // empty). Slice exists with model: null. The mount effect that
    // starts a session short-circuits in this branch, so the seed
    // effect must compensate. `agentChatGetSession` resolves null →
    // provider default.
    const existingPane = { ...pane, thread_id: "thread-x" };
    render(<AgentChatPane pane={existingPane} />);
    await waitFor(() => expect(setModelMock).toHaveBeenCalled());
    expect(agentChatGetSession).toHaveBeenCalledWith("thread-x");
    const [threadId, model] = setModelMock.mock.calls[0];
    expect(threadId).toBe("thread-x");
    // defaultModelForProvider("claude") falls back to "claude-opus-4-8"
    // when capabilities aren't in the test environment.
    expect(model).toBe("claude-opus-4-8");
  });

  it("seeds the persisted model over the Opus default", async () => {
    // Bug #2: after restart the picker must show the user's chosen
    // model, not the Opus default. The persisted row wins.
    vi.mocked(agentChatGetSession).mockResolvedValue(
      makeRecord({ model: "claude-sonnet-4-6" }),
    );
    const existingPane = { ...pane, thread_id: "thread-x" };
    render(<AgentChatPane pane={existingPane} />);
    await waitFor(() => expect(setModelMock).toHaveBeenCalled());
    const [, model] = setModelMock.mock.calls[0];
    expect(model).toBe("claude-sonnet-4-6");
  });

  it("seeds effort / contextWindow / permissionMode from the record", async () => {
    vi.mocked(agentChatGetSession).mockResolvedValue(
      makeRecord({
        model: "claude-sonnet-4-6",
        effort: "high",
        context_window: "1m",
        permission_mode: "plan",
      }),
    );
    const existingPane = { ...pane, thread_id: "thread-x" };
    render(<AgentChatPane pane={existingPane} />);
    await waitFor(() =>
      expect(setEffortMock).toHaveBeenCalledWith("thread-x", "high"),
    );
    expect(setContextWindowMock).toHaveBeenCalledWith("thread-x", "1m");
    expect(setPermissionModeMock).toHaveBeenCalledWith("thread-x", "plan");
  });

  it("restores a persisted Codex mode over the legacy Claude slice sentinel", async () => {
    vi.mocked(agentChatGetSession).mockResolvedValue(
      makeRecord({
        provider: "codex",
        model: "gpt-5.4",
        permission_mode: "workspace-write",
      }),
    );
    const codexPane = {
      ...pane,
      thread_id: "thread-x",
      provider: "codex" as const,
    };
    render(<AgentChatPane pane={codexPane} />);
    await waitFor(() =>
      expect(setPermissionModeMock).toHaveBeenCalledWith(
        "thread-x",
        "workspace-write",
      ),
    );
  });

  it("does not clobber a permission-mode change the user made during the in-flight fetch", async () => {
    // Finding 2: the post-fetch re-check must guard EACH field, not just
    // `model`. Here the slice's model is still null (so the effect
    // proceeds and seeds the model), but the user changed permission mode
    // to "acceptEdits" while `agent_chat_get_session` was in flight. The
    // stale persisted "plan" must NOT be written back over it.
    currentSliceOverrides = { "thread-x": { permissionMode: "acceptEdits" } };
    vi.mocked(agentChatGetSession).mockResolvedValue(
      makeRecord({ model: "claude-sonnet-4-6", permission_mode: "plan" }),
    );
    const existingPane = { ...pane, thread_id: "thread-x" };
    render(<AgentChatPane pane={existingPane} />);
    // The model still seeds (the user didn't touch it).
    await waitFor(() =>
      expect(setModelMock).toHaveBeenCalledWith(
        "thread-x",
        "claude-sonnet-4-6",
      ),
    );
    // ...but the user's in-flight permission-mode pick is preserved.
    expect(setPermissionModeMock).not.toHaveBeenCalledWith("thread-x", "plan");
  });

  it("restores resumeCursor from the persisted sdk_session_id", async () => {
    // Bug #1 support: a picker-triggered silent restart must resume the
    // durable SDK session rather than start fresh, so the mount seed
    // rebuilds { resume: sdk_session_id } into the slice.
    vi.mocked(agentChatGetSession).mockResolvedValue(
      makeRecord({ model: "claude-sonnet-4-6", sdk_session_id: "sdk-abc" }),
    );
    const existingPane = { ...pane, thread_id: "thread-x" };
    render(<AgentChatPane pane={existingPane} />);
    await waitFor(() => expect(setResumeCursorMock).toHaveBeenCalled());
    expect(setResumeCursorMock).toHaveBeenCalledWith("thread-x", {
      resume: "sdk-abc",
    });
  });

  it("seeds resumeCursor null when the row has no sdk_session_id", async () => {
    vi.mocked(agentChatGetSession).mockResolvedValue(
      makeRecord({ model: "claude-sonnet-4-6", sdk_session_id: null }),
    );
    const existingPane = { ...pane, thread_id: "thread-x" };
    render(<AgentChatPane pane={existingPane} />);
    await waitFor(() => expect(setResumeCursorMock).toHaveBeenCalled());
    expect(setResumeCursorMock).toHaveBeenCalledWith("thread-x", null);
  });

  it("does not fetch the session row when there is no thread_id yet", () => {
    // The "new pane" mount path takes a different branch — it starts
    // a session and the .then() handler seeds the model. The seed
    // effect must not fire its fetch pre-thread, otherwise we'd read
    // (and later write) the wrong key. Assert synchronously: the seed
    // effect calls `agentChatGetSession` before its first await, so a
    // guard failure surfaces without yielding — and yielding here would
    // race the null-thread start path, which legitimately seeds a model
    // once its own session resolves.
    const draftlessPane = {
      ...pane,
      thread_id: null as unknown as string,
    };
    render(<AgentChatPane pane={draftlessPane} />);
    expect(agentChatGetSession).not.toHaveBeenCalled();
  });

  it("uses codex default when the pane is on the codex provider", async () => {
    // The fallback table maps each provider to its own default. A
    // pane on codex must not get seeded with the claude default.
    const codexPane = {
      ...pane,
      thread_id: "thread-x",
      provider: "codex" as const,
    };
    render(<AgentChatPane pane={codexPane} />);
    await waitFor(() => expect(setModelMock).toHaveBeenCalled());
    const [, model] = setModelMock.mock.calls[0];
    expect(model).toBe("gpt-5.4");
  });
});

describe("AgentChatPane picker-config persistence (design G)", () => {
  beforeEach(() => {
    currentMessages = [];
    currentThreadsMap = {};
    currentDraftsById = {};
    currentSliceOverrides = {};
    workspaceIdForPaneOverride = "ws-home";
    vi.mocked(agentChatGetSession).mockReset().mockResolvedValue(null);
    vi.mocked(agentChatUpdateSessionConfig).mockClear();
    mockAppState.appState = HOME_APP_STATE.appState;
  });
  afterEach(() => {
    cleanup();
    useProviderCapabilities.setState({ codex: null, codexError: null });
  });

  it("persists the model on a picker change (fire-and-forget)", async () => {
    const existingPane = { ...pane, thread_id: "thread-x" };
    const { container } = render(<AgentChatPane pane={existingPane} />);
    const btn = container.querySelector(
      '[data-testid="model-change"]',
    ) as HTMLButtonElement;
    btn.click();
    await waitFor(() =>
      expect(agentChatUpdateSessionConfig).toHaveBeenCalled(),
    );
    const [threadId, config] = vi.mocked(agentChatUpdateSessionConfig).mock
      .calls[0];
    expect(threadId).toBe("thread-x");
    expect(config).toMatchObject({ model: "claude-sonnet-4-6" });
  });

  it("persists the context window on a picker change", async () => {
    const existingPane = { ...pane, thread_id: "thread-x" };
    const { container } = render(<AgentChatPane pane={existingPane} />);
    const btn = container.querySelector(
      '[data-testid="context-window-change"]',
    ) as HTMLButtonElement;
    btn.click();
    await waitFor(() =>
      expect(agentChatUpdateSessionConfig).toHaveBeenCalledWith("thread-x", {
        context_window: "1m",
      }),
    );
  });

  it("applies Codex Fast mode live without stopping or restarting the session", async () => {
    useProviderCapabilities.setState({
      codex: {
        models: [
          {
            id: "gpt-5.4",
            label: "GPT-5.4",
            description: null,
            effort_levels: ["medium", "high"],
            default_effort: "medium",
            effort_descriptions: {},
            prompt_injected_effort_levels: [],
            context_window_options: [],
            supports_adaptive_thinking: false,
            supports_thinking_toggle: false,
            supports_fast_mode: true,
            supports_images: true,
            sub_provider: null,
            is_free: false,
          },
        ],
        effort_granularity: "per_turn",
        effort_label_map: {},
        permission_modes: [],
        default_permission_mode: "danger-full-access",
        permission_granularity: "per_session",
      },
      codexError: null,
    });
    currentSliceOverrides = {
      "thread-x": { model: "gpt-5.4", fastMode: false },
    };
    const codexPane = {
      ...pane,
      provider: "codex" as const,
      thread_id: "thread-x",
    };
    const { container } = render(<AgentChatPane pane={codexPane} />);
    vi.mocked(agentChatSetFastMode).mockClear();
    vi.mocked(agentChatStopSession).mockClear();
    vi.mocked(agentChatStartSession).mockClear();

    const button = container.querySelector(
      '[data-testid="fast-mode-change"]',
    ) as HTMLButtonElement;
    button.click();

    await waitFor(() =>
      expect(agentChatSetFastMode).toHaveBeenCalledWith(
        "codex",
        "thread-x",
        true,
      ),
    );
    expect(setFastModeMock).toHaveBeenCalledWith("thread-x", true);
    expect(agentChatStopSession).not.toHaveBeenCalled();
    expect(agentChatStartSession).not.toHaveBeenCalled();
  });
});

const grokPane = { ...pane, provider: "grok" as const };

function grokModel(
  id: string,
  efforts: string[] = ["high", "low"],
  defaultEffort: string | null = efforts[0] ?? null,
): ChatModelInfo {
  return {
    id,
    label: id,
    description: null,
    effort_levels: efforts,
    default_effort: defaultEffort,
    effort_descriptions: {},
    prompt_injected_effort_levels: [],
    context_window_options: [],
    supports_adaptive_thinking: false,
    supports_thinking_toggle: efforts.length > 0,
    supports_fast_mode: false,
    supports_images: false,
    sub_provider: null,
    max_context_tokens: 500_000,
    is_free: false,
  };
}

function seedGrokCapabilities(
  models: ChatModelInfo[] = [grokModel("grok-4.6")],
): void {
  useProviderCapabilities.setState({
    grok: {
      models,
      effort_granularity: "per_turn",
      effort_label_map: {},
      permission_modes: [
        {
          value: "agent",
          label: "Agent",
          description: "Provider-controlled approvals.",
          is_default: true,
        },
      ],
      default_permission_mode: "agent",
      permission_granularity: "per_session",
    },
    grokError: null,
  });
}

describe("AgentChatPane Grok live capability reconciliation", () => {
  beforeEach(() => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    currentThreadsMap = {};
    currentDraftsById = {};
    currentSliceOverrides = {};
    workspaceIdForPaneOverride = "ws-home";
    vi.mocked(agentChatGetSession).mockReset().mockResolvedValue(null);
    vi.mocked(agentChatSetModel).mockReset().mockResolvedValue(undefined);
    vi.mocked(agentChatRespondToRequest).mockReset().mockResolvedValue(
      undefined,
    );
    vi.mocked(agentChatStartSession)
      .mockReset()
      .mockResolvedValue("thread-x");
    vi.mocked(agentChatStopSession).mockReset().mockResolvedValue(undefined);
    vi.mocked(agentChatSendTurn).mockClear();
    vi.mocked(agentChatSetPermissionMode).mockClear();
    vi.mocked(agentChatUpdateSessionConfig)
      .mockReset()
      .mockResolvedValue(undefined);
    setModelMock.mockReset();
    setEffortMock.mockReset();
    setContextWindowMock.mockReset();
    setFastModeMock.mockReset();
    setModeMock.mockClear();
    setModePriorMock.mockClear();
    setPermissionModeMock.mockClear();
    seedGrokCapabilities();
  });

  afterEach(() => {
    cleanup();
    useProviderCapabilities.setState({ grok: null, grokError: null });
    vi.mocked(agentChatSetModel).mockReset().mockResolvedValue(undefined);
    vi.mocked(agentChatRespondToRequest).mockReset().mockResolvedValue(
      undefined,
    );
    vi.mocked(agentChatStartSession)
      .mockReset()
      .mockResolvedValue("thread-new");
    vi.mocked(agentChatStopSession).mockReset().mockResolvedValue(undefined);
    vi.mocked(agentChatUpdateSessionConfig)
      .mockReset()
      .mockResolvedValue(undefined);
    setModelMock.mockReset();
    setEffortMock.mockReset();
    setContextWindowMock.mockReset();
    setFastModeMock.mockReset();
  });

  it("reselects the live model when a retired effort falls back to the advertised default", async () => {
    currentSliceOverrides = {
      "thread-x": { model: "grok-4.6", effort: "retired-effort" },
    };

    render(<AgentChatPane pane={grokPane} />);

    await waitFor(() =>
      expect(setEffortMock).toHaveBeenCalledWith("thread-x", "high"),
    );
    expect(agentChatUpdateSessionConfig).toHaveBeenCalledWith("thread-x", {
      effort: "high",
    });
    // Omitting `_meta.reasoningEffort` on Grok's session/set_model makes
    // the CLI resolve its current catalogue default for this model.
    expect(agentChatSetModel).toHaveBeenCalledWith(
      "grok",
      "thread-x",
      "grok-4.6",
    );
  });

  it("keeps a rejected retired-effort reconciliation rolled back after rerender", async () => {
    currentSliceOverrides = {
      "thread-x": { model: "grok-4.6", effort: "retired-effort" },
    };
    setEffortMock.mockImplementation((threadId: string, value: string | null) => {
      currentSliceOverrides[threadId] = {
        ...currentSliceOverrides[threadId],
        effort: value,
      };
    });
    vi.mocked(agentChatSetModel).mockRejectedValueOnce(
      new Error("provider rejected effort reset"),
    );

    const view = render(<AgentChatPane pane={grokPane} />);
    await waitFor(() =>
      expect(setEffortMock).toHaveBeenLastCalledWith(
        "thread-x",
        "retired-effort",
      ),
    );

    view.rerender(<AgentChatPane pane={grokPane} />);
    await Promise.resolve();

    expect(agentChatSetModel).toHaveBeenCalledTimes(1);
    expect(setEffortMock).toHaveBeenCalledTimes(2);
    expect(setEffortMock).toHaveBeenLastCalledWith(
      "thread-x",
      "retired-effort",
    );
    expect(agentChatUpdateSessionConfig).toHaveBeenLastCalledWith(
      "thread-x",
      { effort: "retired-effort" },
    );
  });

  it("moves a retired model to the first live model with one coherent compatibility patch", async () => {
    currentSliceOverrides = {
      "thread-x": {
        model: "grok-retired",
        effort: "xhigh",
        contextWindow: "1m",
      },
    };
    seedGrokCapabilities([grokModel("grok-future")]);

    render(<AgentChatPane pane={grokPane} />);

    await waitFor(() =>
      expect(agentChatSetModel).toHaveBeenCalledWith(
        "grok",
        "thread-x",
        "grok-future",
      ),
    );
    expect(setModelMock).toHaveBeenCalledWith("thread-x", "grok-future");
    expect(setEffortMock).toHaveBeenCalledWith("thread-x", "high");
    expect(setContextWindowMock).toHaveBeenCalledWith("thread-x", null);
    expect(agentChatUpdateSessionConfig).toHaveBeenCalledWith("thread-x", {
      model: "grok-future",
      effort: "high",
      context_window: null,
    });
  });

  it("starts a fresh native Grok session for an incompatible model family while preserving the Codemux thread", async () => {
    currentSliceOverrides = {
      "thread-x": {
        model: "grok-4.6",
        effort: "low",
        resumeCursor: { resume: "native-grok-session" },
      },
    };
    seedGrokCapabilities([
      grokModel("grok-4.6"),
      grokModel("grok-future"),
    ]);
    setModelMock.mockImplementation((threadId: string, value: string | null) => {
      currentSliceOverrides[threadId] = {
        ...currentSliceOverrides[threadId],
        model: value,
      };
    });
    vi.mocked(agentChatSetModel).mockRejectedValueOnce(
      new Error("grok_model_restart_required: incompatible agent family"),
    );

    const { getByTestId } = render(<AgentChatPane pane={grokPane} />);
    fireEvent.click(getByTestId("grok-model-change"));

    await waitFor(() =>
      expect(agentChatStartSession).toHaveBeenCalledWith(
        "pane-1",
        "grok",
        expect.objectContaining({
          thread_id: "thread-x",
          model: "grok-future",
          resume_cursor: null,
          fresh_session: true,
          effort: "low",
        }),
      ),
    );
    expect(agentChatStopSession).toHaveBeenCalledWith("grok", "thread-x");
  });

  it("ignores an obsolete incompatible-family rejection after a newer model succeeds", async () => {
    currentSliceOverrides = {
      "thread-x": { model: "grok-4.6", effort: "high" },
    };
    seedGrokCapabilities([
      grokModel("grok-4.6"),
      grokModel("grok-a"),
      grokModel("grok-b"),
    ]);
    setModelMock.mockImplementation((threadId: string, value: string | null) => {
      currentSliceOverrides[threadId] = {
        ...currentSliceOverrides[threadId],
        model: value,
      };
    });
    let rejectFirst!: (reason?: unknown) => void;
    vi.mocked(agentChatSetModel)
      .mockImplementationOnce(
        () =>
          new Promise<void>((_resolve, reject) => {
            rejectFirst = reject;
          }),
      )
      .mockResolvedValueOnce(undefined);

    const { getByTestId } = render(<AgentChatPane pane={grokPane} />);
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    vi.mocked(agentChatStartSession).mockClear();
    vi.mocked(agentChatStopSession).mockClear();

    fireEvent.click(getByTestId("grok-model-change-a"));
    fireEvent.click(getByTestId("grok-model-change-b"));
    expect(agentChatSetModel).toHaveBeenNthCalledWith(
      1,
      "grok",
      "thread-x",
      "grok-a",
    );
    expect(agentChatSetModel).toHaveBeenNthCalledWith(
      2,
      "grok",
      "thread-x",
      "grok-b",
    );

    await act(async () => {
      rejectFirst(
        new Error("grok_model_restart_required: incompatible agent family"),
      );
      await Promise.resolve();
    });

    expect(currentSliceOverrides["thread-x"]?.model).toBe("grok-b");
    expect(agentChatStopSession).not.toHaveBeenCalled();
    expect(agentChatStartSession).not.toHaveBeenCalled();
  });

  it("freezes Grok configuration during an active turn", async () => {
    currentSliceOverrides = {
      "thread-x": { model: "grok-4.6", effort: "high", streaming: true },
    };
    seedGrokCapabilities([
      grokModel("grok-4.6"),
      grokModel("grok-future"),
    ]);

    const { getByTestId } = render(<AgentChatPane pane={grokPane} />);
    expect(getByTestId("composer")).toHaveAttribute(
      "data-configuration-ready",
      "false",
    );
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    vi.mocked(agentChatSetModel).mockClear();
    vi.mocked(agentChatSetPermissionMode).mockClear();
    vi.mocked(agentChatStartSession).mockClear();
    vi.mocked(agentChatStopSession).mockClear();
    setModelMock.mockClear();
    setPermissionModeMock.mockClear();
    setContextWindowMock.mockClear();
    setModeMock.mockClear();

    fireEvent.click(getByTestId("grok-model-change"));
    fireEvent.click(getByTestId("provider-model-change"));
    fireEvent.click(getByTestId("permission-mode-change"));
    fireEvent.click(getByTestId("context-window-change"));
    fireEvent.click(getByTestId("mode-activate-debug"));

    await act(async () => {
      await Promise.resolve();
    });
    expect(agentChatSetModel).not.toHaveBeenCalled();
    expect(agentChatSetPermissionMode).not.toHaveBeenCalled();
    expect(agentChatStopSession).not.toHaveBeenCalled();
    expect(agentChatStartSession).not.toHaveBeenCalled();
    expect(setModelMock).not.toHaveBeenCalled();
    expect(setPermissionModeMock).not.toHaveBeenCalled();
    expect(setContextWindowMock).not.toHaveBeenCalled();
    expect(setModeMock).not.toHaveBeenCalled();
  });

  it("rolls model, effort, context, and persistence back when a live model change fails", async () => {
    currentSliceOverrides = {
      "thread-x": {
        model: "grok-4.6",
        effort: "xhigh",
        contextWindow: "1m",
      },
    };
    const currentModel = {
      ...grokModel("grok-4.6", ["xhigh", "high"], "xhigh"),
      context_window_options: [
        {
          value: "1m",
          label: "1M",
          is_default: true,
          context_window_tokens: 1_000_000,
        },
      ],
    };
    seedGrokCapabilities([
      currentModel,
      grokModel("grok-future", ["high"], "high"),
    ]);
    setModelMock.mockImplementation((threadId: string, value: string | null) => {
      currentSliceOverrides[threadId] = {
        ...currentSliceOverrides[threadId],
        model: value,
      };
    });
    setEffortMock.mockImplementation((threadId: string, value: string | null) => {
      currentSliceOverrides[threadId] = {
        ...currentSliceOverrides[threadId],
        effort: value,
      };
    });
    setContextWindowMock.mockImplementation(
      (threadId: string, value: string | null) => {
        currentSliceOverrides[threadId] = {
          ...currentSliceOverrides[threadId],
          contextWindow: value,
        };
      },
    );
    vi.mocked(agentChatSetModel).mockRejectedValueOnce(
      new Error("provider rejected model"),
    );

    const { getByTestId } = render(<AgentChatPane pane={grokPane} />);
    fireEvent.click(getByTestId("grok-model-change"));

    await waitFor(() => {
      expect(setModelMock).toHaveBeenLastCalledWith("thread-x", "grok-4.6");
      expect(setEffortMock).toHaveBeenLastCalledWith("thread-x", "xhigh");
      expect(setContextWindowMock).toHaveBeenLastCalledWith(
        "thread-x",
        "1m",
      );
    });
    expect(agentChatUpdateSessionConfig).toHaveBeenLastCalledWith(
      "thread-x",
      {
        model: "grok-4.6",
        effort: "xhigh",
        context_window: "1m",
        fast_mode: false,
      },
    );
  });

  it("rolls the picker back when the restart-required recovery cannot run", async () => {
    // `grok_model_restart_required` recovery depends on the restart helper
    // to make the pick real. When the helper declines — another restart
    // owns the session — the optimistic pick must not be left showing a
    // model the native session never accepted.
    currentSliceOverrides = {
      "thread-x": { model: "grok-4.6", effort: "low" },
    };
    seedGrokCapabilities([grokModel("grok-4.6"), grokModel("grok-future")]);
    setModelMock.mockImplementation((threadId: string, value: string | null) => {
      currentSliceOverrides[threadId] = {
        ...currentSliceOverrides[threadId],
        model: value,
      };
    });
    let rejectSetModel!: (reason?: unknown) => void;
    vi.mocked(agentChatSetModel).mockImplementationOnce(
      () =>
        new Promise<void>((_resolve, reject) => {
          rejectSetModel = reject;
        }),
    );
    // Hold the provider switch open so its restart guard is still owned
    // when the stale model change reports back.
    vi.mocked(agentChatStopSession).mockImplementationOnce(
      () => new Promise<void>(() => {}),
    );

    const { getByTestId } = render(<AgentChatPane pane={grokPane} />);
    await act(async () => {
      await Promise.resolve();
    });
    fireEvent.click(getByTestId("grok-model-change"));
    expect(currentSliceOverrides["thread-x"]?.model).toBe("grok-future");
    fireEvent.click(getByTestId("provider-model-change"));
    await act(async () => {
      await Promise.resolve();
    });

    await act(async () => {
      rejectSetModel(
        new Error("grok_model_restart_required: incompatible agent family"),
      );
      await Promise.resolve();
    });

    expect(setModelMock).toHaveBeenLastCalledWith("thread-x", "grok-4.6");
    expect(agentChatUpdateSessionConfig).toHaveBeenLastCalledWith("thread-x", {
      model: "grok-4.6",
      effort: "low",
      context_window: null,
      fast_mode: false,
    });
    expect(agentChatStartSession).not.toHaveBeenCalledWith(
      "pane-1",
      "grok",
      expect.objectContaining({ fresh_session: true }),
    );
  });

  it("does not expose Plan or Ask as client-controlled Grok modes", () => {
    currentSliceOverrides = {
      "thread-x": { model: "grok-4.6", permissionMode: "agent" },
    };
    const { getByTestId } = render(<AgentChatPane pane={grokPane} />);

    fireEvent.click(getByTestId("mode-activate-plan"));
    fireEvent.click(getByTestId("mode-activate-ask"));

    expect(agentChatSetPermissionMode).not.toHaveBeenCalled();
    expect(agentChatStartSession).not.toHaveBeenCalled();
    expect(agentChatStopSession).not.toHaveBeenCalled();
    expect(setModeMock).not.toHaveBeenCalled();
    expect(setPermissionModeMock).not.toHaveBeenCalled();
  });

  it("answers Grok's native plan gate directly without synthetic turns or mode changes", async () => {
    currentSliceOverrides = {
      "thread-x": { model: "grok-4.6", permissionMode: "agent" },
    };
    const { getByTestId } = render(<AgentChatPane pane={grokPane} />);

    fireEvent.click(getByTestId("accept-plan"));
    await waitFor(() =>
      expect(agentChatRespondToRequest).toHaveBeenCalledWith(
        "grok",
        "thread-x",
        "req-1",
        { decision: "allow" },
      ),
    );
    expect(agentChatSendTurn).not.toHaveBeenCalled();
    expect(agentChatSetPermissionMode).not.toHaveBeenCalled();

    vi.mocked(agentChatRespondToRequest).mockClear();
    fireEvent.click(getByTestId("reject-plan"));
    await waitFor(() =>
      expect(agentChatRespondToRequest).toHaveBeenCalledWith(
        "grok",
        "thread-x",
        "req-1",
        { decision: "deny", message: "Please revise the plan." },
      ),
    );
    expect(agentChatSendTurn).not.toHaveBeenCalled();
    expect(setModeMock).not.toHaveBeenCalled();
  });
});

describe("AgentChatPane Stop preserves its durable thread", () => {
  beforeEach(() => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    currentThreadsMap = {};
    currentDraftsById = {};
    currentSliceOverrides = {
      "thread-x": { streaming: true },
    };
    workspaceIdForPaneOverride = "ws-home";
    vi.mocked(agentChatInterruptTurn).mockClear().mockResolvedValue(undefined);
    vi.mocked(agentChatStartSession).mockClear();
    vi.mocked(agentChatStopSession).mockClear().mockResolvedValue(undefined);
  });

  afterEach(() => cleanup());

  it.each(["claude", "codex", "grok", "opencode"] as const)(
    "interrupts %s without stopping, restarting, or replacing the thread",
    async (provider) => {
      const { getByTestId } = render(
        <AgentChatPane pane={{ ...pane, provider }} />,
      );

      fireEvent.click(getByTestId("composer-stop"));

      await waitFor(() =>
        expect(agentChatInterruptTurn).toHaveBeenCalledWith(
          provider,
          "thread-x",
          null,
        ),
      );
      expect(agentChatStopSession).not.toHaveBeenCalled();
      expect(agentChatStartSession).not.toHaveBeenCalled();
      expect(getByTestId("transcript")).toHaveAttribute(
        "data-thread-key",
        "thread-x",
      );
    },
  );

  it("blocks a silent restart while the interrupt is still in flight, and allows one once it settles", async () => {
    // The auto-heal effects (stale Fast mode, retired Grok model) call the
    // restart helper from a capability tick, not from a click — so nothing
    // upstream guarantees the user isn't mid-Stop. If a restart slipped
    // through here, Stop's own `finally` would clear `restarting` and hand
    // the composer back a "ready" session that is still being rebuilt.
    let releaseInterrupt!: () => void;
    vi.mocked(agentChatInterruptTurn).mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          releaseInterrupt = () => resolve();
        }),
    );
    const { getByTestId } = render(<AgentChatPane pane={pane} />);

    fireEvent.click(getByTestId("composer-stop"));
    await act(async () => {
      await Promise.resolve();
    });
    expect(getByTestId("composer")).toHaveAttribute(
      "data-session-ready",
      "false",
    );

    fireEvent.click(getByTestId("context-window-change"));
    await act(async () => {
      await Promise.resolve();
    });
    expect(agentChatStopSession).not.toHaveBeenCalled();
    expect(agentChatStartSession).not.toHaveBeenCalled();

    await act(async () => {
      releaseInterrupt();
      await Promise.resolve();
    });
    await waitFor(() =>
      expect(getByTestId("composer")).toHaveAttribute(
        "data-session-ready",
        "true",
      ),
    );

    // The guard is released, not leaked: the same picker change works
    // normally once the interrupt has settled.
    fireEvent.click(getByTestId("context-window-change"));
    await waitFor(() =>
      expect(agentChatStopSession).toHaveBeenCalledWith("claude", "thread-x"),
    );
  });

  // Stop has to be an escape hatch even when the provider cannot be
  // reached. Without a local settle the pane is trapped: the interrupt
  // fails, no provider settlement event ever arrives, `streaming` stays
  // true — which hides the Continue chip and re-renders the Stop button
  // that just failed. This matters most for a run the liveness probe is
  // holding open on a subagent that never reported a terminal snapshot.
  it("settles the thread locally when the interrupt fails", async () => {
    applyEventMock.mockClear();
    vi.mocked(agentChatInterruptTurn)
      .mockClear()
      .mockRejectedValue(new Error("session not found"));

    const { getByTestId } = render(<AgentChatPane pane={pane} />);
    fireEvent.click(getByTestId("composer-stop"));

    await waitFor(() => expect(agentChatInterruptTurn).toHaveBeenCalled());
    await waitFor(() =>
      expect(applyEventMock).toHaveBeenCalledWith("thread-x", {
        type: "session_state_changed",
        thread_id: "thread-x",
        status: { status: "ready" },
      }),
    );
  });

  it("leaves settling to the provider when the interrupt succeeds", async () => {
    applyEventMock.mockClear();
    vi.mocked(agentChatInterruptTurn).mockClear().mockResolvedValue(undefined);

    const { getByTestId } = render(<AgentChatPane pane={pane} />);
    fireEvent.click(getByTestId("composer-stop"));

    await waitFor(() => expect(agentChatInterruptTurn).toHaveBeenCalled());
    expect(applyEventMock).not.toHaveBeenCalledWith(
      "thread-x",
      expect.objectContaining({ type: "session_state_changed" }),
    );
  });
});

describe("AgentChatPane provider handoff", () => {
  beforeEach(() => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    currentThreadsMap = {};
    currentDraftsById = {};
    currentSliceOverrides = {
      "thread-x": {
        model: "claude-sonnet-4-6",
        permissionMode: "bypassPermissions",
        resumeCursor: { resume: "claude-sdk-session" },
      },
    };
    workspaceIdForPaneOverride = "ws-home";
    vi.mocked(agentChatGetSession).mockReset().mockResolvedValue(null);
    vi.mocked(agentChatStartSession)
      .mockReset()
      .mockResolvedValue("thread-x");
    vi.mocked(agentChatStopSession)
      .mockReset()
      .mockResolvedValue(undefined);
    setModelMock.mockClear();
    setEffortMock.mockClear();
    setContextWindowMock.mockClear();
    setResumeCursorMock.mockClear();
    setPermissionModeMock.mockClear();
  });

  afterEach(() => cleanup());

  it("atomically switches a restored pane to the selected provider and model", async () => {
    const { container } = render(<AgentChatPane pane={pane} />);

    fireEvent.click(
      container.querySelector('[data-testid="provider-model-change"]')!,
    );

    await waitFor(() =>
      expect(vi.mocked(agentChatStartSession)).toHaveBeenCalledTimes(1),
    );
    expect(vi.mocked(agentChatStopSession)).toHaveBeenCalledWith(
      "claude",
      "thread-x",
    );
    expect(vi.mocked(agentChatStartSession)).toHaveBeenCalledWith(
      "pane-1",
      "codex",
      expect.objectContaining({
        thread_id: "thread-x",
        cwd: "/home/user",
        model: "gpt-5.4",
        resume_cursor: null,
        permission_mode: "danger-full-access",
        effort: null,
        context_window: null,
      }),
    );
    await waitFor(() =>
      expect(
        container.querySelector('[data-testid="composer"]'),
      ).toHaveAttribute("data-provider", "codex"),
    );
    expect(setModelMock).toHaveBeenCalledWith("thread-x", "gpt-5.4");
    expect(setResumeCursorMock).toHaveBeenCalledWith("thread-x", null);
  });

  it("restores the previous provider when the selected provider cannot start", async () => {
    vi.mocked(agentChatStartSession)
      .mockRejectedValueOnce(new Error("codex unavailable"))
      .mockResolvedValueOnce("thread-x");
    const { container } = render(<AgentChatPane pane={pane} />);

    fireEvent.click(
      container.querySelector('[data-testid="provider-model-change"]')!,
    );

    await waitFor(() =>
      expect(vi.mocked(agentChatStartSession)).toHaveBeenCalledTimes(2),
    );
    expect(vi.mocked(agentChatStartSession).mock.calls[0][1]).toBe("codex");
    expect(vi.mocked(agentChatStartSession).mock.calls[1]).toEqual([
      "pane-1",
      "claude",
      expect.objectContaining({
        thread_id: "thread-x",
        model: "claude-sonnet-4-6",
        resume_cursor: { resume: "claude-sdk-session" },
        permission_mode: "bypassPermissions",
      }),
    ]);
    expect(
      container.querySelector('[data-testid="composer"]'),
    ).toHaveAttribute("data-provider", "claude");
  });
});

// ─────────────────────────────────────────────────────────────────────
// Mode pill removal — the SDK rejects live `setPermissionMode` calls
// that would land on `bypassPermissions` even when the session was
// originally launched with `--dangerously-skip-permissions`. The
// removal handler therefore restores the prior mode via a silent
// session restart instead of the live setter. These tests guard that
// routing decision and the slice cleanup that follows.
// ─────────────────────────────────────────────────────────────────────

describe("AgentChatPane handleModeRemove silent-restart", () => {
  beforeEach(() => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    currentThreadsMap = {};
    currentDraftsById = {};
    currentSliceOverrides = {};
    workspaceIdForPaneOverride = "ws-home";
    setModeMock.mockClear();
    setModePriorMock.mockClear();
    setPermissionModeMock.mockClear();
    setModelMock.mockClear();
    markRequestResolvedMock.mockClear();
    vi.mocked(agentChatStartSession).mockClear().mockResolvedValue("thread-x");
    vi.mocked(agentChatStopSession).mockClear().mockResolvedValue(undefined);
    vi.mocked(agentChatSetPermissionMode).mockClear().mockResolvedValue(undefined);
  });
  afterEach(() => cleanup());

  it("removing Ask pill with prior bypassPermissions restarts the session — does NOT call agentChatSetPermissionMode", async () => {
    currentSliceOverrides = {
      "thread-x": {
        mode: "ask",
        modePriorPermissionMode: "bypassPermissions",
        permissionMode: "plan", // session is currently in plan
        sessionLaunchMode: "plan",
      },
    };

    const { container } = render(<AgentChatPane pane={pane} />);
    const removeBtn = container.querySelector(
      '[data-testid="mode-remove"]',
    ) as HTMLButtonElement;
    removeBtn.click();
    // restartSessionWith fires-and-forgets an async IIFE — yield once
    // to let the agentChatStartSession await resolve.
    await Promise.resolve();
    await Promise.resolve();

    // The bug we are guarding against: live setter would reject with
    // "Cannot set permission mode to bypassPermissions because the
    // session was not launched with --dangerously-skip-permissions".
    expect(agentChatSetPermissionMode).not.toHaveBeenCalled();
    // Silent restart re-launches with the prior mode in launch params
    // so the SDK re-applies `--dangerously-skip-permissions`.
    expect(agentChatStartSession).toHaveBeenCalled();
    const startInput = vi.mocked(agentChatStartSession).mock.calls[0][2];
    expect(startInput.thread_id).toBe("thread-x");
    expect(startInput.permission_mode).toBe("bypassPermissions");
    // UI snap: slice.permissionMode flips immediately, slice.mode
    // returns to "default", priorPermissionMode is cleared.
    expect(setPermissionModeMock).toHaveBeenCalledWith(
      "thread-x",
      "bypassPermissions",
    );
    expect(setModeMock).toHaveBeenCalledWith("thread-x", "default");
    expect(setModePriorMock).toHaveBeenCalledWith("thread-x", null);
  });

  it("removing Plan pill with prior 'default' uses the same restart pattern", async () => {
    currentSliceOverrides = {
      "thread-x": {
        mode: "plan",
        modePriorPermissionMode: "default",
        permissionMode: "plan",
        sessionLaunchMode: "plan",
      },
    };

    const { container } = render(<AgentChatPane pane={pane} />);
    const removeBtn = container.querySelector(
      '[data-testid="mode-remove"]',
    ) as HTMLButtonElement;
    removeBtn.click();
    await Promise.resolve();
    await Promise.resolve();

    expect(agentChatSetPermissionMode).not.toHaveBeenCalled();
    expect(agentChatStartSession).toHaveBeenCalled();
    const startInput = vi.mocked(agentChatStartSession).mock.calls[0][2];
    expect(startInput.permission_mode).toBe("default");
    expect(setPermissionModeMock).toHaveBeenCalledWith("thread-x", "default");
    expect(setModeMock).toHaveBeenCalledWith("thread-x", "default");
  });

  it("activating Debug pill flips slice.mode without an SDK setPermissionMode call", async () => {
    const { container } = render(<AgentChatPane pane={pane} />);
    const activateBtn = container.querySelector(
      '[data-testid="mode-activate-debug"]',
    ) as HTMLButtonElement;
    activateBtn.click();
    await Promise.resolve();
    await Promise.resolve();

    expect(agentChatSetPermissionMode).not.toHaveBeenCalled();
    expect(setModeMock).toHaveBeenCalledWith("thread-x", "debug");
    expect(setPermissionModeMock).not.toHaveBeenCalledWith(
      "thread-x",
      "plan",
    );
  });

  it("activating the Plan pill re-persists the durable permission mode so a restart isn't left plan-locked (finding 6)", async () => {
    // The pill flips the LIVE session to read-only "plan" (in-memory
    // only), but `agent_chat_set_permission_mode` just wrote "plan" to
    // the DB row. Without the re-persist, a restart auto-resumes a
    // read-only "plan" session with the pill gone and the prior mode
    // unrecoverable. The handler must write the user's DURABLE mode back
    // to the DB.
    vi.mocked(agentChatUpdateSessionConfig).mockClear().mockResolvedValue(
      undefined,
    );
    currentSliceOverrides = { "thread-x": { permissionMode: "acceptEdits" } };
    const { container } = render(<AgentChatPane pane={pane} />);
    const activateBtn = container.querySelector(
      '[data-testid="mode-activate-plan"]',
    ) as HTMLButtonElement;
    activateBtn.click();

    await waitFor(() =>
      expect(agentChatSetPermissionMode).toHaveBeenCalledWith(
        "claude",
        "thread-x",
        "plan",
      ),
    );
    await waitFor(() =>
      expect(agentChatUpdateSessionConfig).toHaveBeenCalledWith("thread-x", {
        permission_mode: "acceptEdits",
      }),
    );
  });

  it("regression guard: handleAcceptPlan still uses the live setPermissionMode (plan → default is allowed by the SDK)", async () => {
    currentSliceOverrides = {
      "thread-x": {
        mode: "plan",
        modePriorPermissionMode: "bypassPermissions",
        permissionMode: "plan",
        sessionLaunchMode: "plan",
      },
    };

    const { container } = render(<AgentChatPane pane={pane} />);
    const acceptBtn = container.querySelector(
      '[data-testid="accept-plan"]',
    ) as HTMLButtonElement;
    acceptBtn.click();
    await Promise.resolve();
    await Promise.resolve();

    // The accept-plan path deliberately stays on the live setter
    // because the SDK only blocks live transitions TO bypassPermissions
    // — going plan → default is allowed. Routing this through the
    // restart helper would needlessly tear down the session right
    // before the synthetic "Proceed with the plan." turn fires.
    expect(agentChatSetPermissionMode).toHaveBeenCalled();
    const [, , acceptMode] = vi.mocked(agentChatSetPermissionMode).mock.calls[0];
    expect(acceptMode).toBe("default");
    expect(agentChatStartSession).not.toHaveBeenCalled();
    expect(agentChatStopSession).not.toHaveBeenCalled();
  });
});

// ─────────────────────────────────────────────────────────────────────
// Cursor — the first provider that combines real permission modes with
// PER-TURN granularity, and the first whose plan approval is a blocking
// RPC rather than a synthetic follow-up turn.
// ─────────────────────────────────────────────────────────────────────

const cursorPane = { ...pane, provider: "cursor" as const };

function seedCursorCapabilities() {
  useProviderCapabilities.setState({
    cursor: {
      models: [],
      effort_granularity: "per_turn",
      effort_label_map: {},
      permission_modes: [
        {
          value: "ask",
          label: "Ask first",
          description: "Ask before commands or edits that need approval.",
          is_default: false,
        },
        {
          value: "agent",
          label: "Full access",
          description: "Work without approval prompts.",
          is_default: true,
        },
      ],
      default_permission_mode: "agent",
      permission_granularity: "per_turn",
    },
    cursorError: null,
  });
}

describe("AgentChatPane — Cursor per-turn permission + plan accept", () => {
  beforeEach(() => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    currentThreadsMap = {};
    currentDraftsById = {};
    currentSliceOverrides = {};
    workspaceIdForPaneOverride = "ws-home";
    setModeMock.mockClear();
    setModePriorMock.mockClear();
    setPermissionModeMock.mockClear();
    vi.mocked(agentChatSetPermissionMode).mockClear().mockResolvedValue(
      undefined,
    );
    vi.mocked(agentChatStartSession).mockClear();
    vi.mocked(agentChatStopSession).mockClear();
    vi.mocked(agentChatSendTurn).mockClear();
    vi.mocked(agentChatUpdateSessionConfig).mockClear().mockResolvedValue(
      undefined,
    );
    seedCursorCapabilities();
  });
  afterEach(() => {
    cleanup();
    useProviderCapabilities.setState({ cursor: null, cursorError: null });
  });

  it("pushes a per-turn permission mode onto the LIVE session instead of only persisting it", async () => {
    // Persisting alone stranded the choice: every sendTurn call site
    // passes `permission_mode_override: null`, so flipping Full access →
    // Ask first never reached the adapter and the picker lied for the
    // rest of the session.
    currentSliceOverrides = {
      "thread-x": { permissionMode: "agent", sessionLaunchMode: "agent" },
    };
    const { container } = render(<AgentChatPane pane={cursorPane} />);
    (
      container.querySelector(
        '[data-testid="permission-mode-change"]',
      ) as HTMLButtonElement
    ).click();

    await waitFor(() =>
      expect(agentChatSetPermissionMode).toHaveBeenCalledWith(
        "cursor",
        "thread-x",
        "ask",
      ),
    );
    expect(setPermissionModeMock).toHaveBeenCalledWith("thread-x", "ask");
    await waitFor(() =>
      expect(agentChatUpdateSessionConfig).toHaveBeenCalledWith("thread-x", {
        permission_mode: "ask",
      }),
    );
    // Per-turn granularity means no session teardown.
    expect(agentChatStopSession).not.toHaveBeenCalled();
    expect(agentChatStartSession).not.toHaveBeenCalled();
  });

  it("accepting a Cursor plan drops the read-only Plan pill and restores the prior mode", async () => {
    // Answering the create_plan RPC is consent to IMPLEMENT the plan.
    // Leaving `mode: "plan"` in the slice kept the composer showing the
    // read-only Plan pill while Cursor edited files.
    currentSliceOverrides = {
      "thread-x": {
        mode: "plan",
        modePriorPermissionMode: "agent",
        permissionMode: "plan",
        sessionLaunchMode: "plan",
      },
    };
    const { container } = render(<AgentChatPane pane={cursorPane} />);
    (
      container.querySelector('[data-testid="accept-plan"]') as HTMLButtonElement
    ).click();

    await waitFor(() =>
      expect(agentChatSetPermissionMode).toHaveBeenCalledWith(
        "cursor",
        "thread-x",
        "agent",
      ),
    );
    expect(setModeMock).toHaveBeenCalledWith("thread-x", "default");
    expect(setModePriorMock).toHaveBeenCalledWith("thread-x", null);
    expect(setPermissionModeMock).toHaveBeenCalledWith("thread-x", "agent");
    // Cursor applies the mode live — no restart, and no synthetic
    // "Proceed with the plan." turn (the RPC answer is the go-ahead).
    expect(agentChatStopSession).not.toHaveBeenCalled();
    expect(agentChatSendTurn).not.toHaveBeenCalled();
  });

  it("sanitizes a foreign stashed mode against Cursor's own table", async () => {
    // A thread handed over from Claude carries Claude's vocabulary in
    // `modePriorPermissionMode`. Restoring it verbatim made the adapter
    // reject the mode and the session stayed read-only.
    currentSliceOverrides = {
      "thread-x": {
        mode: "plan",
        modePriorPermissionMode: "bypassPermissions",
        permissionMode: "plan",
        sessionLaunchMode: "plan",
      },
    };
    const { container } = render(<AgentChatPane pane={cursorPane} />);
    (
      container.querySelector('[data-testid="accept-plan"]') as HTMLButtonElement
    ).click();

    await waitFor(() =>
      expect(agentChatSetPermissionMode).toHaveBeenCalledWith(
        "cursor",
        "thread-x",
        // Cursor's advertised default, not Claude's Full-access spelling.
        "agent",
      ),
    );
  });

  it("keeps the Plan pill when the live mode change fails", async () => {
    // Clearing the pill first would unlock the composer while the
    // session was still read-only — the desync this branch exists to
    // prevent, inverted.
    vi.mocked(agentChatSetPermissionMode).mockRejectedValueOnce(
      new Error("session gone"),
    );
    currentSliceOverrides = {
      "thread-x": {
        mode: "plan",
        modePriorPermissionMode: "agent",
        permissionMode: "plan",
        sessionLaunchMode: "plan",
      },
    };
    const { container } = render(<AgentChatPane pane={cursorPane} />);
    (
      container.querySelector('[data-testid="accept-plan"]') as HTMLButtonElement
    ).click();

    await waitFor(() => expect(agentChatSetPermissionMode).toHaveBeenCalled());
    // The pill survives: mode stays "plan" and the stash is not dropped,
    // so the user can retry (or exit via the pill) instead of facing a
    // composer that lies about being writable.
    expect(setModeMock).not.toHaveBeenCalledWith("thread-x", "default");
    expect(setModePriorMock).not.toHaveBeenCalledWith("thread-x", null);
  });
});

// ─────────────────────────────────────────────────────────────────────
// Stage 6 — Debug-mode cleanup affordances. These tests cover the
// grep-on-mount hook that seeds hasDebugActivity, the exit-dialog
// branch on pill-removal, the cleanup turn synthesis, and the banner
// visibility gate.
// ─────────────────────────────────────────────────────────────────────

describe("AgentChatPane Stage 6 — Debug-mode cleanup", () => {
  beforeEach(() => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    currentThreadsMap = {};
    currentDraftsById = {};
    currentSliceOverrides = {};
    workspaceIdForPaneOverride = "ws-home";
    setModeMock.mockClear();
    setHasDebugActivityMock.mockClear();
    setDebugActivityResolvedMock.mockClear();
    lastDebugExitOpen = false;
    lastDebugExitOnChoose = null;
    vi.mocked(grepCountPattern).mockClear().mockResolvedValue(0);
    vi.mocked(agentChatSendTurn).mockClear().mockResolvedValue({ turn_id: "turn-1", queued_id: null });
  });
  afterEach(() => cleanup());

  it("grep-on-mount fires with the workspace project root and CODEMUX_DEBUG pattern", async () => {
    render(<AgentChatPane pane={pane} />);
    await Promise.resolve();
    expect(grepCountPattern).toHaveBeenCalledWith(
      "/home/user",
      "CODEMUX_DEBUG",
    );
  });

  it("grep-on-mount with hits flips hasDebugActivity true and marks resolved", async () => {
    vi.mocked(grepCountPattern).mockResolvedValue(3);
    render(<AgentChatPane pane={pane} />);
    await Promise.resolve();
    await Promise.resolve();
    expect(setHasDebugActivityMock).toHaveBeenCalledWith("thread-x", true);
    expect(setDebugActivityResolvedMock).toHaveBeenCalledWith(
      "thread-x",
      true,
    );
  });

  it("grep-on-mount failure soft-fails: hasDebugActivity stays false, resolved still flips", async () => {
    vi.mocked(grepCountPattern).mockRejectedValue(new Error("rg missing"));
    render(<AgentChatPane pane={pane} />);
    await Promise.resolve();
    await Promise.resolve();
    expect(setHasDebugActivityMock).toHaveBeenCalledWith("thread-x", false);
    expect(setDebugActivityResolvedMock).toHaveBeenCalledWith(
      "thread-x",
      true,
    );
  });

  it("Debug-pill removal with no detected markers skips the dialog and just flips slice.mode", async () => {
    currentSliceOverrides = {
      "thread-x": {
        mode: "debug",
        hasDebugActivity: false,
        debugActivityResolved: true,
      },
    };
    const { container } = render(<AgentChatPane pane={pane} />);
    const removeBtn = container.querySelector(
      '[data-testid="mode-remove"]',
    ) as HTMLButtonElement;
    removeBtn.click();
    await Promise.resolve();
    await Promise.resolve();

    expect(lastDebugExitOpen).toBe(false);
    expect(setModeMock).toHaveBeenCalledWith("thread-x", "default");
  });

  it("Debug-pill removal with markers opens the exit dialog and pauses on Cancel", async () => {
    currentSliceOverrides = {
      "thread-x": {
        mode: "debug",
        hasDebugActivity: true,
        debugActivityResolved: true,
      },
    };
    const { container } = render(<AgentChatPane pane={pane} />);
    const removeBtn = container.querySelector(
      '[data-testid="mode-remove"]',
    ) as HTMLButtonElement;
    removeBtn.click();
    await Promise.resolve();
    expect(lastDebugExitOpen).toBe(true);
    // Cancel keeps the pill: setMode("default") never fires for this thread.
    setModeMock.mockClear();
    lastDebugExitOnChoose?.("cancel");
    await Promise.resolve();
    await Promise.resolve();
    expect(setModeMock).not.toHaveBeenCalledWith("thread-x", "default");
  });

  it("Debug-pill removal — Leave them path drops the pill without firing a cleanup turn", async () => {
    currentSliceOverrides = {
      "thread-x": {
        mode: "debug",
        hasDebugActivity: true,
        debugActivityResolved: true,
      },
    };
    const { container } = render(<AgentChatPane pane={pane} />);
    const removeBtn = container.querySelector(
      '[data-testid="mode-remove"]',
    ) as HTMLButtonElement;
    removeBtn.click();
    await Promise.resolve();
    setModeMock.mockClear();
    lastDebugExitOnChoose?.("leave");
    await Promise.resolve();
    await Promise.resolve();

    expect(setModeMock).toHaveBeenCalledWith("thread-x", "default");
    expect(agentChatSendTurn).not.toHaveBeenCalled();
  });

  it("Debug-pill removal — Remove markers path fires the cleanup turn and clears hasDebugActivity", async () => {
    currentSliceOverrides = {
      "thread-x": {
        mode: "debug",
        hasDebugActivity: true,
        debugActivityResolved: true,
      },
    };
    const { container } = render(<AgentChatPane pane={pane} />);
    const removeBtn = container.querySelector(
      '[data-testid="mode-remove"]',
    ) as HTMLButtonElement;
    removeBtn.click();
    await Promise.resolve();
    lastDebugExitOnChoose?.("cleanup");
    // Two awaits for setMode + send-turn promise + finally.
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();

    // Cleanup flips slice.mode to default first so the wrapper isn't
    // re-applied to the cleanup prompt itself, then sends the turn.
    expect(setModeMock).toHaveBeenCalledWith("thread-x", "default");
    expect(agentChatSendTurn).toHaveBeenCalled();
    const [, sendInput] = vi.mocked(agentChatSendTurn).mock.calls[0];
    expect(sendInput.text).toContain("CODEMUX_DEBUG");
    expect(setHasDebugActivityMock).toHaveBeenCalledWith("thread-x", false);
  });

  it("DebugCleanupBanner only renders when mode=debug AND hasDebugActivity AND debugActivityResolved", async () => {
    // Combo 1: all three true → banner visible.
    currentSliceOverrides = {
      "thread-x": {
        mode: "debug",
        hasDebugActivity: true,
        debugActivityResolved: true,
      },
    };
    const { container, rerender } = render(<AgentChatPane pane={pane} />);
    expect(
      container.querySelector('[data-testid="debug-cleanup-banner"]'),
    ).not.toBeNull();

    // Combo 2: resolution still pending → banner hidden.
    currentSliceOverrides = {
      "thread-x": {
        mode: "debug",
        hasDebugActivity: true,
        debugActivityResolved: false,
      },
    };
    rerender(<AgentChatPane pane={pane} />);
    expect(
      container.querySelector('[data-testid="debug-cleanup-banner"]'),
    ).toBeNull();

    // Combo 3: mode=debug, resolved=true, but hasDebugActivity=false
    // (no markers in workspace) → banner hidden.
    currentSliceOverrides = {
      "thread-x": {
        mode: "debug",
        hasDebugActivity: false,
        debugActivityResolved: true,
      },
    };
    rerender(<AgentChatPane pane={pane} />);
    expect(
      container.querySelector('[data-testid="debug-cleanup-banner"]'),
    ).toBeNull();

    // Combo 4: mode=default with markers → still hidden (banner is
    // strictly debug-mode-only — it's an exit affordance, not a global
    // janitor).
    currentSliceOverrides = {
      "thread-x": {
        mode: "default",
        hasDebugActivity: true,
        debugActivityResolved: true,
      },
    };
    rerender(<AgentChatPane pane={pane} />);
    expect(
      container.querySelector('[data-testid="debug-cleanup-banner"]'),
    ).toBeNull();
  });

  it("triggerDebugCleanup is single-fire — clicking the banner twice in the same tick fires only one cleanup turn", async () => {
    currentSliceOverrides = {
      "thread-x": {
        mode: "debug",
        hasDebugActivity: true,
        debugActivityResolved: true,
      },
    };
    // Slow-walk the send-turn promise so the second click definitely
    // lands while the first is still in flight.
    const sendDeferred: {
      resolve: ((v: { turn_id: string; queued_id: string | null }) => void) | null;
    } = {
      resolve: null,
    };
    vi.mocked(agentChatSendTurn).mockImplementation(
      () =>
        new Promise<{ turn_id: string; queued_id: string | null }>((resolve) => {
          sendDeferred.resolve = resolve;
        }),
    );
    const { container } = render(<AgentChatPane pane={pane} />);
    const banner = container.querySelector(
      '[data-testid="debug-cleanup-banner"]',
    ) as HTMLButtonElement;
    expect(banner).not.toBeNull();
    // Two synchronous clicks — same tick. The ref guard must reject
    // the second.
    banner.click();
    banner.click();
    expect(agentChatSendTurn).toHaveBeenCalledTimes(1);
    sendDeferred.resolve?.({ turn_id: "turn-done", queued_id: null });
  });

  it("cleanup ordering: setMode('default') is called BEFORE agentChatSendTurn so the wrapper does not re-instruct", async () => {
    currentSliceOverrides = {
      "thread-x": {
        mode: "debug",
        hasDebugActivity: true,
        debugActivityResolved: true,
      },
    };
    const callOrder: string[] = [];
    setModeMock.mockImplementation((_tid: string, mode: string) => {
      if (mode === "default") callOrder.push("setMode(default)");
    });
    vi.mocked(agentChatSendTurn).mockImplementation(async () => {
      callOrder.push("agentChatSendTurn");
      return { turn_id: "turn-1", queued_id: null };
    });

    const { container } = render(<AgentChatPane pane={pane} />);
    const removeBtn = container.querySelector(
      '[data-testid="mode-remove"]',
    ) as HTMLButtonElement;
    removeBtn.click();
    await Promise.resolve();
    lastDebugExitOnChoose?.("cleanup");
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();

    const setModeIdx = callOrder.indexOf("setMode(default)");
    const sendTurnIdx = callOrder.indexOf("agentChatSendTurn");
    expect(setModeIdx).toBeGreaterThanOrEqual(0);
    expect(sendTurnIdx).toBeGreaterThanOrEqual(0);
    expect(setModeIdx).toBeLessThan(sendTurnIdx);
  });

  it("grep cancellation: the cancelled-flag cleanup prevents stale slice writes after unmount", async () => {
    // Hold the grep promise so we can unmount BEFORE it resolves.
    const grepDeferred: { resolve: ((v: number) => void) | null } = {
      resolve: null,
    };
    vi.mocked(grepCountPattern).mockImplementation(
      () =>
        new Promise<number>((resolve) => {
          grepDeferred.resolve = resolve;
        }),
    );
    const { unmount } = render(<AgentChatPane pane={pane} />);
    // Effect ran and called setDebugActivityResolved(threadId, false)
    // synchronously to mark "in flight". Clear the spy so we only
    // observe writes that happen AFTER unmount.
    setHasDebugActivityMock.mockClear();
    setDebugActivityResolvedMock.mockClear();
    unmount();
    // Now resolve the grep — the cleanup ran, `cancelled` is true,
    // and neither setter should fire.
    grepDeferred.resolve?.(7);
    await Promise.resolve();
    await Promise.resolve();
    expect(setHasDebugActivityMock).not.toHaveBeenCalled();
    expect(setDebugActivityResolvedMock).not.toHaveBeenCalled();
  });
});
