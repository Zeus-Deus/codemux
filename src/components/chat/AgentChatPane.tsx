import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { useAgentChatEvents } from "@/hooks/use-agent-chat-events";
import {
  planCapabilityCompatReset,
  planEffortChange,
  planModelChange,
  planPermissionModeChange,
  planSubmit,
} from "@/lib/agent-chat/chat-pane-plans";
import {
  buildAttachmentBlock,
  buildFileResolvedContent,
  buildFolderResolvedContent,
  buildImagePayloads,
  buildIssueResolvedContent,
  buildPrResolvedContent,
} from "@/lib/agent-chat/attachment-block";
import { activeAttachments } from "@/lib/agent-chat/attachment-tokens";
import { applyAllPrefixes } from "@/lib/agent-chat/mode-prefix";
import { resolveSkillBodies } from "@/lib/agent-chat/skill-tokens";
import {
  selectActiveSkills,
  useSkillsStore,
} from "@/stores/skills-store";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import { hasUltrathinkInBodyText } from "@/lib/agent-chat/ultrathink";
import { basename } from "@/lib/path";
import { toast } from "@/lib/toast";
import {
  findWorkspaceIdForPane,
  groupWorkspacesByProject,
  useAppStore,
  useHomeDir,
} from "@/stores/app-store";
import {
  DEFAULT_THREAD_PERMISSION_MODE,
  useAgentChatStore,
  type Attachment,
} from "@/stores/agent-chat-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useShallow } from "zustand/react/shallow";
import {
  selectCapabilities,
  selectModel,
  useProviderCapabilities,
} from "@/stores/provider-capabilities-store";
import {
  activateWorkspace,
  agentChatInterruptTurn,
  agentChatListMessages,
  agentChatRespondToRequest,
  agentChatSendTurn,
  agentChatSetModel,
  agentChatSetPermissionMode,
  agentChatStartSession,
  agentChatStopSession,
  checkGhStatus,
  checkGithubRepo,
  getGithubIssueByPath,
  getGithubPrByPath,
  getGithubPrDiffByPath,
  grepCountPattern,
  readFileForAttachment,
  readFolderForAttachment,
} from "@/tauri/commands";
import { replayPayloads } from "@/lib/agent-chat/hydrate";
import { prestartWorktreeSession } from "@/lib/agent-chat/prestart-worktree-session";
import type { AgentChatEventPayload, ApprovalDecision } from "@/tauri/events";
import type {
  AgentChatProviderKind,
  FileMatch,
  FolderMatch,
  GitHubIssue,
  PaneNodeSnapshot,
  PullRequestInfo,
} from "@/tauri/types";

import { ChatTranscript } from "./ChatTranscript";
import { ChatHomeLanding } from "./ChatHomeLanding";
import { Composer } from "./Composer";
import { DebugCleanupBanner } from "./DebugCleanupBanner";
import { DebugExitDialog, type DebugExitChoice } from "./DebugExitDialog";
import {
  type AskUserQuestionOutput,
  ComposerPendingInputPanel,
} from "./ComposerPendingInputPanel";
import { defaultModelForProvider } from "./pickers/ModelPicker";
import { DerivativeBranchPicker } from "./pickers/DerivativeBranchPicker";
import type { ActivePillMode } from "./pickers/ModePill";
import { WorktreePicker } from "./pickers/WorktreePicker";
import { ProjectPicker } from "@/components/overlays/project-picker";
import { useUIStore } from "@/stores/ui-store";

// Kept for parity with Step 1's export shape. The pane tree renderer
// passes the pane snapshot verbatim; nothing else imports this type.
type AgentChatPaneNode = Extract<PaneNodeSnapshot, { kind: "agent_chat" }>;

// Step 12 Stage 4 — flipped on. The picker is now the unified
// `MultiProviderModelPicker` (provider rail + searchable model list)
// that replaces the side-by-side `ProviderPicker + ModelPicker` pair.
// Codex now ships proper `permission_modes` so the original
// `set_permission_mode` concern is moot; OpenCode is federated through
// `sub_provider`. The picker is the only entry point for provider
// switching from inside an existing pane.
const ENABLE_PROVIDER_PICKER = true;

/** Step 8 Stage 7 — hard cap on staged attachments. Above this we
 *  toast and reject the next attach so prompts can't silently grow
 *  into a token-budget cliff. The matching soft-warning copy lives
 *  in Composer.tsx (rendering concern); this constant gates the
 *  attach handlers. */
const ATTACHMENT_HARD_LIMIT = 20;
/** Step 8 Stage 7 — issue/PR fetches go stale at this age. On send,
 *  we re-fetch any GitHub-kind attachment whose `fetchedAt` is older
 *  so the agent always sees fresh detail (state flips, new comments)
 *  even if the user kept the picker open for a few minutes before
 *  hitting Enter. Lines up with the gh detail cache TTL on the Rust
 *  side so a re-fetch is cheap when the cached value is still warm. */
const STALE_ATTACHMENT_THRESHOLD_MS = 60_000;

/** Synthetic user prompt fired when the user invokes Debug-mode
 *  cleanup. The grep pattern carries no comment-prefix so the model
 *  finds CODEMUX_DEBUG markers across every language Claude touched. */
const CLEANUP_PROMPT = `Search for \`CODEMUX_DEBUG\` across the project and remove every line containing that marker. Also remove any surrounding debug-only scaffolding (variable declarations, imports) that were added solely to support those markers. After removing, run a final grep to verify zero matches remain.`;

/** Step 8 Stage 7 — animated GIF detection. Anthropic's image API
 *  rejects animated GIFs (only the first frame is read, and that
 *  often produces confusing errors). We detect at attach time and
 *  reject with a clear toast rather than letting the SDK surface a
 *  cryptic 400. The GIF89a spec frames each animation cycle with a
 *  Graphic Control Extension marker (`0x21 0xF9 0x04`); a static
 *  GIF has 0–1, an animated GIF has 2+. The scan walks the entire
 *  file but bails early on the second hit so even multi-MB animated
 *  GIFs cost milliseconds. */
export function detectAnimatedGif(buffer: ArrayBuffer): boolean {
  const bytes = new Uint8Array(buffer);
  let frameCount = 0;
  for (let i = 0; i < bytes.length - 3; i++) {
    if (
      bytes[i] === 0x21 &&
      bytes[i + 1] === 0xf9 &&
      bytes[i + 2] === 0x04
    ) {
      frameCount++;
      if (frameCount > 1) return true;
    }
  }
  return false;
}

export function AgentChatPane({ pane }: { pane: AgentChatPaneNode }) {
  const initialProvider: AgentChatProviderKind = pane.provider ?? "claude";
  const [provider, setProvider] = useState<AgentChatProviderKind>(initialProvider);
  const [threadId, setThreadId] = useState<string | null>(pane.thread_id);
  const [starting, setStarting] = useState(false);
  const [restarting, setRestarting] = useState(false);
  // Optimistic in-flight flag mirroring the reference impl's
  // `isSendBusy` (ChatView.tsx:406). Set synchronously on submit so
  // the button disables BEFORE the backend's Running event
  // round-trips.
  //
  // NOTE: `isSending` is useState (drives the render), but rapid-fire
  // Enter presses in the SAME JS tick don't see the updated state —
  // the `useCallback` closure captured the pre-set snapshot. The
  // reference impl pairs the state with a `sendInFlightRef.current`
  // synchronous guard (ChatView.tsx:2458) for exactly this reason:
  // refs mutate synchronously, so the second call within the same
  // tick sees the flag the first one just set. We do the same.
  const [isSending, setIsSending] = useState(false);
  const sendInFlightRef = useRef(false);

  // Stage 6 Debug-mode cleanup affordances. The exit dialog opens when
  // the user removes the Debug pill while CODEMUX_DEBUG markers exist;
  // the busy flag disables the in-banner Clean up button while the
  // synthetic cleanup turn is in flight.
  const [debugExitDialog, setDebugExitDialog] = useState<{
    resolve: (choice: DebugExitChoice) => void;
  } | null>(null);
  // The `cleanupInFlight` state drives the banner's busy label; the
  // ref is the synchronous race guard. Same trick as `sendInFlightRef`
  // — React state updates aren't visible to a second click that fires
  // in the same tick (the closure already captured `cleanupInFlight =
  // false`), so the ref provides the actual single-fire guarantee.
  const [cleanupInFlight, setCleanupInFlight] = useState(false);
  const cleanupInFlightRef = useRef(false);

  const fallbackCwd = useAppStore((s) => {
    if (!s.appState) return null;
    const ws = s.appState.workspaces.find(
      (w) => w.workspace_id === s.appState!.active_workspace_id,
    );
    return ws?.cwd ?? null;
  });
  const cwd = pane.cwd ?? fallbackCwd;

  // Step 8 Stage 4 — preflight GitHub status. We check on mount + on
  // cwd change so a workspace swap doesn't leave the popup acting on
  // a stale answer. `null` means "not yet known" — the popup keeps
  // GitHub entries disabled while we wait so the user never sees the
  // entry flip from enabled to disabled. Errors fall back to `false`
  // (treat as not-a-github-repo) which gives the user a reachable
  // disabled-state instead of a crashing popup.
  const [isGithubRepo, setIsGithubRepo] = useState<boolean | null>(null);
  const [ghAuthenticated, setGhAuthenticated] = useState<boolean | null>(null);
  useEffect(() => {
    if (!cwd) {
      setIsGithubRepo(false);
      setGhAuthenticated(null);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const [repo, gh] = await Promise.all([
          checkGithubRepo(cwd),
          checkGhStatus(),
        ]);
        if (cancelled) return;
        setIsGithubRepo(repo);
        setGhAuthenticated(gh.status === "Authenticated");
      } catch {
        if (cancelled) return;
        setIsGithubRepo(false);
        setGhAuthenticated(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [cwd]);

  // Stage C race fix: when the pane was just created by
  // `materializeAndSend` → `agent_chat_create_pane`, its `thread_id`
  // starts as `null` and only flips to the draft's pre-minted value
  // once the `agent_chat_start_session` emit lands on the frontend.
  // If mount happens inside that race window, the original `useState`
  // initialiser captured `null` and the mount effect below would mint
  // a fresh thread id + start a duplicate session — orphaning the
  // slice materialise seeded with the draft's thread id. This
  // prop-sync effect catches up whenever `pane.thread_id` becomes
  // non-null after mount.
  useEffect(() => {
    if (pane.thread_id && pane.thread_id !== threadId) {
      setThreadId(pane.thread_id);
    }
  }, [pane.thread_id, threadId]);

  // Stage C race fix (belt to the above suspender): if `pane.thread_id`
  // is still null at mount but a promoted draft claims this workspace,
  // use the draft's pre-minted thread id directly. Materialize already
  // started the session server-side — AgentChatPane just subscribes.
  const workspaceIdForPane = useAppStore((s) =>
    findWorkspaceIdForPane(s, pane.pane_id),
  );
  const promotedDraftThreadId = useChatDraftStore((s) => {
    if (!workspaceIdForPane) return null;
    // Match must be both workspace AND pane-scoped. A pure workspace
    // match would hand the FIRST pane's threadId to a freshly-created
    // SECOND pane in the same workspace (e.g. a Chat Agent preset
    // click on a workspace already hosting a chat), making the two
    // panes mirror the same Zustand slice. CLI panes don't have this
    // problem because each gets a uniquely-minted `session_id` —
    // chats need the equivalent invariant scoped per pane id.
    const match = Object.values(s.draftsById).find(
      (d) =>
        d.promotedTo?.workspaceId === workspaceIdForPane &&
        d.promotedTo.paneId === pane.pane_id,
    );
    return match?.threadId ?? null;
  });
  // Recovery fallback: if materialize made it past pane-creation but
  // failed during start_session / send_turn, the draft has
  // `materializedTo` set and `promotedTo` still null. The existing
  // `promotedDraftThreadId` selector above doesn't match — it only
  // looks for fully-promoted drafts. The mount effect below uses this
  // to finish the half-completed materialise: adopt the orphan thread
  // id, start the session that step 3 never managed, and call
  // `markPromoted` so we don't loop.
  // `useShallow` is REQUIRED here. The selector synthesises a fresh
  // object literal whenever a match exists, so default `Object.is`
  // equality would treat every render as a state change and trigger
  // an infinite re-render loop ("Maximum update depth exceeded").
  // `useShallow` does a shallow per-key comparison and returns the
  // previous reference when the field values are unchanged.
  const recoveryDraft = useChatDraftStore(
    useShallow((s) => {
      if (!workspaceIdForPane) return null;
      // Pane-scoped match — see promotedDraftThreadId for the same
      // rationale. A fresh second chat pane in the same workspace
      // must NOT inherit the first pane's partially-materialised
      // recovery state.
      const match = Object.values(s.draftsById).find(
        (d) =>
          d.promotedTo === null &&
          d.materializedTo?.workspaceId === workspaceIdForPane &&
          d.materializedTo.paneId === pane.pane_id,
      );
      if (!match || !match.materializedTo) return null;
      return {
        draftId: match.draftId,
        threadId: match.threadId,
        paneId: match.materializedTo.paneId,
        workspaceId: match.materializedTo.workspaceId,
        provider: match.provider,
        model: match.model,
        permissionMode: match.permissionMode,
        effort: match.effort,
        contextWindow: match.contextWindow,
      };
    }),
  );
  const markDraftPromoted = useChatDraftStore((s) => s.markPromoted);
  const clearDraft = useChatDraftStore((s) => s.clearDraft);
  // A pane is "home-rooted" if its workspace's project_root matches
  // the cached $HOME. This replaces the legacy `workspace_type ===
  // "home"` check since the Home singleton was retired in Stage B of
  // the Home rework.
  const homeDir = useHomeDir();
  const workspaceProjectRoot = useAppStore((s) => {
    if (!s.appState) return null;
    const ws = s.appState.workspaces.find(
      (w) =>
        w.workspace_id === (workspaceIdForPane ?? s.appState!.active_workspace_id),
    );
    return ws?.project_root ?? ws?.cwd ?? null;
  });
  const isHomeWorkspace =
    homeDir !== null && workspaceProjectRoot === homeDir;
  const setShowNewWorkspaceDialog = useUIStore(
    (s) => s.setShowNewWorkspaceDialog,
  );

  const ensureThread = useAgentChatStore((s) => s.ensureThread);
  const setInputDraft = useAgentChatStore((s) => s.setInputDraft);
  const setStoreModel = useAgentChatStore((s) => s.setModel);
  const setStorePermissionMode = useAgentChatStore((s) => s.setPermissionMode);
  const setSessionLaunchMode = useAgentChatStore(
    (s) => s.setSessionLaunchMode,
  );
  const removeStagedAttachment = useAgentChatStore(
    (s) => s.removeStagedAttachment,
  );
  const addStagedAttachment = useAgentChatStore((s) => s.addStagedAttachment);
  const updateStagedAttachment = useAgentChatStore(
    (s) => s.updateStagedAttachment,
  );
  const clearStagedAttachments = useAgentChatStore(
    (s) => s.clearStagedAttachments,
  );
  const setStoreEffort = useAgentChatStore((s) => s.setEffort);
  const setStoreContextWindow = useAgentChatStore((s) => s.setContextWindow);
  const setStoreMode = useAgentChatStore((s) => s.setMode);
  const setStoreModePriorPermissionMode = useAgentChatStore(
    (s) => s.setModePriorPermissionMode,
  );
  const setStoreHasDebugActivity = useAgentChatStore(
    (s) => s.setHasDebugActivity,
  );
  const setStoreDebugActivityResolved = useAgentChatStore(
    (s) => s.setDebugActivityResolved,
  );
  // Skills registry for send-time `/skill-name` token resolution. The
  // store dedupes loads behind a 60s TTL, so reading without a load
  // call is fine — Composer's slash-popup effect ensures the registry
  // is hydrated by the time the user picks a skill.
  // `selectActiveSkills` filters out user-disabled skills so a
  // disabled `/release` token in the textarea is treated as plain
  // prose, not silently injected.
  const skillsRegistry = useSkillsStore(selectActiveSkills);
  const migrateThreadId = useAgentChatStore((s) => s.migrateThreadId);
  const appendUserMessage = useAgentChatStore((s) => s.appendUserMessage);
  const markRequestResponding = useAgentChatStore(
    (s) => s.markRequestResponding,
  );
  const markRequestResolved = useAgentChatStore(
    (s) => s.markRequestResolved,
  );

  // Chat-side capabilities for the active provider. `null` until the
  // refresh hook resolves (or when the backend errors — pickers render
  // a disabled "unavailable" state in that case).
  const capabilities = useProviderCapabilities((s) =>
    selectCapabilities(s, provider),
  );

  const slice = useAgentChatStore((s) =>
    threadId ? s.threads[threadId] : null,
  );
  const draft = slice?.inputDraft ?? "";
  const messages = slice?.messages ?? EMPTY_MESSAGES;
  const streaming = slice?.streaming ?? false;
  const activeTurnId = slice?.activeTurnId ?? null;
  const model = slice?.model ?? null;
  const permissionMode =
    slice?.permissionMode ?? DEFAULT_THREAD_PERMISSION_MODE;
  const effort = slice?.effort ?? null;
  const contextWindow = slice?.contextWindow ?? null;
  const mode = slice?.mode ?? "default";
  const hasDebugActivity = slice?.hasDebugActivity ?? false;
  const debugActivityResolved = slice?.debugActivityResolved ?? false;
  const stagedAttachments = slice?.stagedAttachments ?? EMPTY_ATTACHMENTS;
  const activeModel = selectModel(capabilities, model);
  const effortLabelMap = capabilities?.effort_label_map ?? {};
  const permissionModes = capabilities?.permission_modes ?? null;
  const ultrathinkInBodyText = hasUltrathinkInBodyText(draft);

  // When capabilities arrive (or change), reset the thread's
  // permissionMode if it's orphaned for the active provider. Seeds
  // the default when the slice has no mode yet. Idempotent — fires
  // only when the current value actually needs to change.
  useEffect(() => {
    if (!threadId) return;
    if (!capabilities) return;
    const plan = planCapabilityCompatReset({
      capabilities,
      currentPermissionMode: permissionMode,
    });
    if (plan.resetPermissionMode !== undefined) {
      // null is a legitimate reset value (provider with no modes).
      setStorePermissionMode(
        threadId,
        plan.resetPermissionMode ?? DEFAULT_THREAD_PERMISSION_MODE,
      );
    }
  }, [threadId, capabilities, permissionMode, setStorePermissionMode]);

  // Seed slice.model with the provider's default whenever the slice
  // exists (threadId set) but has no model yet. Three paths land in
  // this state:
  //   (a) app restart — the pane snapshot still carries a thread_id,
  //       but the in-memory store was wiped, so `ensureThread` below
  //       creates a fresh empty slice
  //   (b) resume from session history — `hydrateThread` rebuilds the
  //       transcript from persisted events, none of which carry the
  //       chosen model
  //   (c) any future flow that pre-creates a slice without seeding
  //       the model (e.g. silent restart on a thread that never got
  //       a model assigned)
  // Without this seed, ReasoningPicker (which short-circuits on
  // `!model`) renders nothing and the user loses the effort /
  // context-window picker. Idempotent — bails the moment a model is
  // present.
  useEffect(() => {
    if (!threadId) return;
    if (model !== null) return;
    setStoreModel(threadId, defaultModelForProvider(provider));
  }, [threadId, model, provider, setStoreModel]);

  // Stage 6 grep-on-chat-open: when the pane mounts (or the project
  // root changes), background-check whether the workspace already
  // contains `CODEMUX_DEBUG` markers from a previous debug session.
  // The result drives the cleanup banner / exit dialog. Failures are
  // soft: we treat them as "no markers" so a missing ripgrep binary
  // can't pin the cleanup affordance on forever.
  useEffect(() => {
    if (!threadId) return;
    if (!workspaceProjectRoot) return;
    let cancelled = false;
    setStoreDebugActivityResolved(threadId, false);
    grepCountPattern(workspaceProjectRoot, "CODEMUX_DEBUG")
      .then((count) => {
        if (cancelled) return;
        setStoreHasDebugActivity(threadId, count > 0);
        setStoreDebugActivityResolved(threadId, true);
      })
      .catch((err) => {
        console.warn("[agent-chat] debug-marker grep failed:", err);
        if (cancelled) return;
        setStoreHasDebugActivity(threadId, false);
        setStoreDebugActivityResolved(threadId, true);
      });
    return () => {
      cancelled = true;
    };
  }, [
    threadId,
    workspaceProjectRoot,
    setStoreHasDebugActivity,
    setStoreDebugActivityResolved,
  ]);

  // Subscribe to provider events for this thread. The handler reads
  // store actions via `getState()` so it stays stable across
  // re-renders — otherwise we'd rebind the Tauri listener on every
  // keystroke.
  const handleEvent = useCallback((payload: AgentChatEventPayload) => {
    useAgentChatStore.getState().applyEvent(payload.thread_id, payload.event);
  }, []);
  useAgentChatEvents(threadId, handleEvent);

  // Hydrate on (re)mount when the pane attaches to a thread that
  // already has a server-side transcript.
  //
  // The provider event broadcaster is live-only (bounded channel, no
  // replay — see docs/features/agent-chat.md). When the user switches
  // workspaces the entire inactive workspace's pane tree unmounts
  // (workspace-main.tsx returns null for non-active workspaces), so
  // the `useAgentChatEvents` listener detaches and any provider events
  // emitted while the pane is gone are dropped on the frontend side.
  // The backend still persists transcript-affecting events to SQLite
  // via `forward_event` in `commands/agent_chat.rs`, but nothing pulled
  // that history back into the in-memory Zustand slice on remount —
  // so the user came back to a chat showing only their optimistic
  // user message with no agent reply, even though the agent had run
  // (file changes proved it).
  //
  // We pull the persisted transcript on every mount that lands with a
  // truthy `threadId`, replay it through the same pure reducer the
  // live stream uses, and only call `hydrateThread` if the replayed
  // transcript is longer than the in-memory slice — otherwise the
  // local state is at least as fresh as disk and a hydrate would
  // clobber events that haven't been persisted yet (the persistence
  // path is async, so live state can briefly lead disk).
  //
  // `hydrateAttemptedRef` is keyed by thread id so a thread switch
  // (resume → new thread id, recovery → adopted orphan thread id)
  // re-triggers hydrate, but a stable thread id within one mount only
  // hydrates once.
  const hydrateAttemptedRef = useRef<string | null>(null);
  useEffect(() => {
    if (!threadId) return;
    if (hydrateAttemptedRef.current === threadId) return;
    hydrateAttemptedRef.current = threadId;
    let cancelled = false;
    void (async () => {
      try {
        const payloads = await agentChatListMessages(threadId);
        if (cancelled) return;
        if (payloads.length === 0) return;
        const replayed = replayPayloads(payloads);
        const slice = useAgentChatStore.getState().threads[threadId];
        const localCount = slice?.messages.length ?? 0;
        // Guard against clobbering live state: only hydrate when disk
        // has strictly more rendered messages than memory. Equal /
        // shorter means the live stream kept up.
        if (replayed.messages.length <= localCount) return;
        useAgentChatStore.getState().hydrateThread(threadId, payloads);
      } catch (err) {
        // Soft-fail: if hydrate fails, the user still sees whatever
        // the live stream brings in. Log so it's debuggable.
        console.warn("[agent-chat] hydrate-on-mount failed:", err);
      }
    })();
    return () => {
      cancelled = true;
      // Release the attempt marker on teardown. Without this, React
      // StrictMode's dev-only mount→unmount→remount cycle cancels the
      // first invocation's in-flight fetch and the second invocation
      // bails on the marker — so a dev build never hydrates. The
      // remount re-runs the effect with a clean slate; in production
      // (single mount) this cleanup only runs on real unmounts, where
      // the ref dies anyway.
      if (hydrateAttemptedRef.current === threadId) {
        hydrateAttemptedRef.current = null;
      }
    };
  }, [threadId]);

  // Start a session on mount if the pane doesn't already have one.
  const startAttempted = useRef(false);
  // Tracks whether we've already attempted to recover a partial
  // materialise on this mount. Unlike `startAttempted` (which resets
  // on failure to allow a hot retry), recovery is a one-shot per
  // mount: if it fails, the draft stays in SendFailed state and the
  // user can retry by re-opening the workspace.
  const recoveryAttempted = useRef(false);
  useEffect(() => {
    if (threadId) {
      ensureThread(threadId);
      return;
    }
    // Stage C race fix: if a promoted draft owns this workspace, it
    // already started the session under `draft.threadId`. Just adopt
    // that id — don't spin up a second session.
    if (promotedDraftThreadId) {
      setThreadId(promotedDraftThreadId);
      ensureThread(promotedDraftThreadId);
      return;
    }
    // Recovery: the draft completed step 2 (workspace + pane) but
    // step 3 (start_session) or step 4 (send_turn) failed. Adopt
    // the orphan thread id (which already has the user's optimistic
    // message in its slice) and finish what materialize started.
    if (recoveryDraft && !recoveryAttempted.current) {
      if (starting) return;
      if (!cwd) return;
      recoveryAttempted.current = true;
      startAttempted.current = true;
      setStarting(true);
      const recoveryMode =
        recoveryDraft.permissionMode ?? DEFAULT_THREAD_PERMISSION_MODE;
      const startInput = {
        thread_id: recoveryDraft.threadId,
        cwd,
        model: recoveryDraft.model,
        resume_cursor: null,
        permission_mode: recoveryMode,
        additional_directories: [],
        env: null,
      };
      agentChatStartSession(pane.pane_id, provider, startInput)
        .then((id) => {
          setThreadId(id);
          ensureThread(id);
          if (recoveryDraft.model !== null) {
            setStoreModel(id, recoveryDraft.model);
          } else {
            setStoreModel(id, defaultModelForProvider(provider));
          }
          setStorePermissionMode(id, recoveryMode);
          setSessionLaunchMode(id, recoveryMode);
          // Mark the draft as promoted so subsequent mounts take the
          // existing promotedDraftThreadId branch above instead of
          // re-attempting recovery.
          markDraftPromoted(recoveryDraft.draftId, {
            workspaceId: recoveryDraft.workspaceId,
            paneId: recoveryDraft.paneId,
            threadId: id,
          });
          // Sweep the now-promoted draft on the same 5s grace window
          // the success path uses (DraftChatSurface.tsx, preset-bar.tsx).
          const draftIdToClear = recoveryDraft.draftId;
          setTimeout(() => clearDraft(draftIdToClear), 5000);
        })
        .catch((err) => {
          toast.error(`Failed to recover chat session: ${err}`);
          // Leave the draft in SendFailed state. The user can re-open
          // the workspace to retry, or close it and start over.
        })
        .finally(() => setStarting(false));
      return;
    }
    if (starting || startAttempted.current) return;
    if (!cwd) return;
    startAttempted.current = true;
    setStarting(true);
    const localThreadId = `chat-${pane.pane_id}-${Date.now()}`;
    // For a brand-new thread with no slice yet, use the default mode.
    const startMode = DEFAULT_THREAD_PERMISSION_MODE;
    const startInput = {
      thread_id: localThreadId,
      cwd,
      model: null,
      resume_cursor: null,
      permission_mode: startMode,
      additional_directories: [],
      env: null,
    };
    const defaultModel = defaultModelForProvider(provider);
    agentChatStartSession(pane.pane_id, provider, startInput)
      .then((id) => {
        setThreadId(id);
        ensureThread(id);
        setStoreModel(id, defaultModel);
        setStorePermissionMode(id, startMode);
        setSessionLaunchMode(id, startMode);
      })
      .catch((err) => {
        toast.error(`Failed to start chat session: ${err}`);
        startAttempted.current = false;
      })
      .finally(() => setStarting(false));
  }, [
    threadId,
    promotedDraftThreadId,
    recoveryDraft,
    starting,
    pane.pane_id,
    provider,
    cwd,
    ensureThread,
    setStoreModel,
    setStorePermissionMode,
    setSessionLaunchMode,
    markDraftPromoted,
    clearDraft,
  ]);

  const handleSubmit = useCallback(() => {
    // Synchronous ref check BEFORE any React state reads — closes the
    // same-tick race that the `useState` guard can't (captured closure
    // sees the pre-set snapshot when two Enter presses fire in one
    // tick). Refs mutate synchronously; the second call sees
    // `sendInFlightRef.current === true` and bails.
    if (sendInFlightRef.current) return;
    if (!threadId) return;
    const rawText = draft.trim();
    if (!rawText) return;
    const plan = planSubmit({ rawText, provider, effort });
    sendInFlightRef.current = true;
    setIsSending(true);
    void (async () => {
      // Stage 7 — re-fetch any GitHub-kind chip whose detail is older
      // than STALE_ATTACHMENT_THRESHOLD_MS so the agent sees fresh
      // state (closed → reopened, new comments, label flips). Files
      // are read fresh by the agent itself via the Read tool, so they
      // don't need this. Failures here are non-fatal: we log and
      // proceed with the stale resolved content rather than abort the
      // turn — better to send something than to hard-block on a flaky
      // gh call.
      try {
        const preStale = useAgentChatStore.getState().threads[threadId];
        const staleList = (preStale?.stagedAttachments ?? []).filter((a) => {
          if (a.kind !== "issue" && a.kind !== "pr") return false;
          if (a.metadata.isLoading) return false;
          const fetchedAt = a.metadata.fetchedAt ?? 0;
          return Date.now() - fetchedAt > STALE_ATTACHMENT_THRESHOLD_MS;
        });
        if (staleList.length > 0) {
          const repoPath = cwd ?? "";
          await Promise.all(
            staleList.map(async (att) => {
              try {
                if (att.kind === "issue") {
                  const num = Number.parseInt(att.ref.replace(/^#/, ""), 10);
                  if (!Number.isFinite(num)) return;
                  const detail = await getGithubIssueByPath(repoPath, num);
                  updateStagedAttachment(threadId, att.id, {
                    resolvedContent: buildIssueResolvedContent(detail),
                    metadata: {
                      ...att.metadata,
                      label: `#${detail.number} ${detail.title}`,
                      state: (detail.state.toLowerCase() === "closed"
                        ? "closed"
                        : "open") as "open" | "closed",
                      fetchedAt: Date.now(),
                    },
                  });
                } else {
                  const num = Number.parseInt(att.ref.replace(/^!/, ""), 10);
                  if (!Number.isFinite(num)) return;
                  const fullDiff = att.metadata.expandFullDiff === true;
                  const [detail, diff] = await Promise.all([
                    getGithubPrByPath(repoPath, num),
                    getGithubPrDiffByPath(repoPath, num, fullDiff),
                  ]);
                  const detailUpper = detail.state.toUpperCase();
                  const resolvedState: "open" | "closed" | "merged" | "draft" =
                    detailUpper === "MERGED"
                      ? "merged"
                      : detailUpper === "CLOSED"
                        ? "closed"
                        : detail.is_draft
                          ? "draft"
                          : "open";
                  updateStagedAttachment(threadId, att.id, {
                    resolvedContent: buildPrResolvedContent(detail, diff, {
                      fullDiff,
                    }),
                    metadata: {
                      ...att.metadata,
                      label: `#${detail.number} ${detail.title}`,
                      state: resolvedState,
                      fetchedAt: Date.now(),
                    },
                  });
                }
              } catch (err) {
                console.warn(
                  `[agent-chat] stale re-fetch failed for ${att.ref}`,
                  err,
                );
              }
            }),
          );
        }
      } catch (err) {
        console.warn("[agent-chat] stale re-fetch outer failure", err);
      }
      // Mode wrappers (Stage 4 onward) live SDK-side only — the
      // transcript stores the unwrapped (ultrathink-only) text so users
      // see what they typed, not the framing we layered on top.
      // Parse `/skill-name` tokens out of the raw text and resolve their
      // bodies against the skills registry. Unmatched tokens (typos, or
      // skills not in the registry) silently pass through as plain prose.
      const skillBodies = resolveSkillBodies(rawText, skillsRegistry);
      // Snapshot staged attachments AT submit time so the block we
      // inject reflects exactly what the user has staged. Reading from
      // the live store via getState() avoids stale closure bugs if a
      // chip resolved between the last render and Enter. Step 8 Stage
      // 2.1 — filter to attachments whose `@<basename>` token still
      // appears in the textarea (`rawText`); deleting a token excludes
      // its file from the prompt without an explicit chip-removal.
      const liveSlice = useAgentChatStore.getState().threads[threadId];
      const liveAttachments = liveSlice?.stagedAttachments ?? [];
      const attachmentBlock = buildAttachmentBlock(
        activeAttachments(rawText, liveAttachments),
      );
      // Stage 6 — images travel as native multimodal content blocks at
      // the SDK layer, NOT inside the text body. We pass the resolved
      // bytes through `images` on SendTurnInput; the Rust adapters
      // translate to provider-specific shapes (Claude `image/base64`,
      // Codex `image_url` data URI).
      const imagePayloads = buildImagePayloads(liveAttachments);
      const sdkText = applyAllPrefixes(
        rawText,
        mode,
        effort,
        skillBodies,
        attachmentBlock,
      );
      appendUserMessage(threadId, plan.text);
      // Clear chips per-turn (matches the inputDraft = "" reset that
      // appendUserMessage already does for the textarea).
      clearStagedAttachments(threadId);
      const input = {
        thread_id: threadId,
        text: sdkText,
        images: imagePayloads,
        model_override: null,
        effort_override: plan.effortOverride,
      };
      try {
        await agentChatSendTurn(provider, input);
      } catch (err) {
        toast.error(`Failed to send turn: ${err}`);
        sendInFlightRef.current = false;
        setIsSending(false);
      }
    })();
  }, [
    threadId,
    draft,
    provider,
    effort,
    mode,
    cwd,
    skillsRegistry,
    appendUserMessage,
    clearStagedAttachments,
    updateStagedAttachment,
  ]);

  /** Step 8 Stage 2 — orchestrates the chip lifecycle when the user
   *  picks a file from the `@` mention popup. The Composer has
   *  already stripped the `@<query>` token from the textarea by the
   *  time this fires, so we just stage the chip with isLoading=true,
   *  fire `read_file_for_attachment`, then patch the chip with the
   *  resolved content (or an error indicator if the read failed). */
  const handleAttachFile = useCallback(
    (match: FileMatch) => {
      if (!threadId) return;
      const liveSlice = useAgentChatStore.getState().threads[threadId];
      if ((liveSlice?.stagedAttachments.length ?? 0) >= ATTACHMENT_HARD_LIMIT) {
        toast.error("Attachment limit reached", {
          description: `Remove some attachments before adding more (max ${ATTACHMENT_HARD_LIMIT}).`,
        });
        return;
      }
      const id =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `att-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const filename = basename(match.path);
      addStagedAttachment(threadId, {
        id,
        kind: "file",
        ref: match.absolute_path,
        metadata: {
          label: filename,
          isLoading: true,
        },
      });
      void (async () => {
        try {
          const info = await readFileForAttachment(match.absolute_path, cwd);
          updateStagedAttachment(threadId, id, {
            resolvedContent: buildFileResolvedContent(info),
            metadata: {
              label: filename,
              lineCount: info.lineCount,
              bytes: info.bytes,
              isTruncated: info.truncated,
              isLoading: false,
              fetchedAt: Date.now(),
            },
          });
        } catch (err) {
          updateStagedAttachment(threadId, id, {
            metadata: {
              label: filename,
              isLoading: false,
              error: String(err),
            },
          });
        }
      })();
    },
    [threadId, cwd, addStagedAttachment, updateStagedAttachment],
  );

  /** Step 8 Stage 3 — folder counterpart to handleAttachFile.
   *  Stages with isLoading=true, fires read_folder_for_attachment
   *  with depth 2 (research-locked), then patches the chip with the
   *  rendered tree. */
  const handleAttachFolder = useCallback(
    (match: FolderMatch) => {
      if (!threadId) return;
      const liveSlice = useAgentChatStore.getState().threads[threadId];
      if ((liveSlice?.stagedAttachments.length ?? 0) >= ATTACHMENT_HARD_LIMIT) {
        toast.error("Attachment limit reached", {
          description: `Remove some attachments before adding more (max ${ATTACHMENT_HARD_LIMIT}).`,
        });
        return;
      }
      const id =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `att-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const folderName = basename(match.path);
      addStagedAttachment(threadId, {
        id,
        kind: "folder",
        ref: match.absolute_path,
        metadata: { label: folderName, isLoading: true },
      });
      void (async () => {
        try {
          const info = await readFolderForAttachment(match.absolute_path, cwd, 2);
          updateStagedAttachment(threadId, id, {
            resolvedContent: buildFolderResolvedContent(info),
            metadata: {
              label: folderName,
              isLoading: false,
              fetchedAt: Date.now(),
            },
          });
        } catch (err) {
          updateStagedAttachment(threadId, id, {
            metadata: {
              label: folderName,
              isLoading: false,
              error: String(err),
            },
          });
        }
      })();
    },
    [threadId, cwd, addStagedAttachment, updateStagedAttachment],
  );

  /** Step 8 Stage 4 — issue counterpart. Stages with isLoading=true,
   *  fires the cached detail fetch, then patches the chip with the
   *  fully resolved body + metadata (state, fetchedAt). Errors render
   *  the chip muted with the error string in the tooltip. */
  const handleAttachIssue = useCallback(
    (summary: GitHubIssue) => {
      if (!threadId) return;
      const liveSlice = useAgentChatStore.getState().threads[threadId];
      if ((liveSlice?.stagedAttachments.length ?? 0) >= ATTACHMENT_HARD_LIMIT) {
        toast.error("Attachment limit reached", {
          description: `Remove some attachments before adding more (max ${ATTACHMENT_HARD_LIMIT}).`,
        });
        return;
      }
      const id =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `att-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const initialState = (summary.state.toLowerCase() === "closed"
        ? "closed"
        : "open") as "open" | "closed";
      addStagedAttachment(threadId, {
        id,
        kind: "issue",
        ref: `#${summary.number}`,
        metadata: {
          label: `#${summary.number} ${summary.title}`,
          state: initialState,
          isLoading: true,
        },
      });
      void (async () => {
        try {
          // Fall back to the workspace cwd when project_root isn't on
          // hand; backend `_by_path` accepts either as long as it's a
          // git checkout the user has gh access to.
          const repoPath = cwd ?? "";
          const detail = await getGithubIssueByPath(repoPath, summary.number);
          updateStagedAttachment(threadId, id, {
            resolvedContent: buildIssueResolvedContent(detail),
            metadata: {
              label: `#${detail.number} ${detail.title}`,
              state: (detail.state.toLowerCase() === "closed"
                ? "closed"
                : "open") as "open" | "closed",
              fetchedAt: Date.now(),
              isLoading: false,
            },
          });
        } catch (err) {
          updateStagedAttachment(threadId, id, {
            metadata: {
              label: `#${summary.number} ${summary.title}`,
              state: initialState,
              isLoading: false,
              error: String(err),
            },
          });
        }
      })();
    },
    [threadId, cwd, addStagedAttachment, updateStagedAttachment],
  );

  /** Step 8 Stage 5 — PR counterpart. Fetches detail + name-only
   *  diff in parallel; resolves the chip with both. The diff is
   *  capped at 100 KB on the Rust side so the prompt budget stays
   *  bounded even for monorepo migrations. */
  const handleAttachPr = useCallback(
    (summary: PullRequestInfo) => {
      if (!threadId) return;
      const liveSlice = useAgentChatStore.getState().threads[threadId];
      if ((liveSlice?.stagedAttachments.length ?? 0) >= ATTACHMENT_HARD_LIMIT) {
        toast.error("Attachment limit reached", {
          description: `Remove some attachments before adding more (max ${ATTACHMENT_HARD_LIMIT}).`,
        });
        return;
      }
      const id =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `att-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const upper = summary.state.toUpperCase();
      const initialState: "open" | "closed" | "merged" | "draft" =
        upper === "MERGED"
          ? "merged"
          : upper === "CLOSED"
            ? "closed"
            : summary.is_draft
              ? "draft"
              : "open";
      addStagedAttachment(threadId, {
        id,
        kind: "pr",
        ref: `!${summary.number}`,
        metadata: {
          label: `#${summary.number} ${summary.title}`,
          state: initialState,
          isLoading: true,
        },
      });
      void (async () => {
        try {
          const repoPath = cwd ?? "";
          // Detail + name-only diff fetched concurrently — both are
          // path-cached separately so a re-pick of the same PR
          // returns from cache after the first call.
          const [detail, diff] = await Promise.all([
            getGithubPrByPath(repoPath, summary.number),
            getGithubPrDiffByPath(repoPath, summary.number, false),
          ]);
          const detailUpper = detail.state.toUpperCase();
          const resolvedState: "open" | "closed" | "merged" | "draft" =
            detailUpper === "MERGED"
              ? "merged"
              : detailUpper === "CLOSED"
                ? "closed"
                : detail.is_draft
                  ? "draft"
                  : "open";
          updateStagedAttachment(threadId, id, {
            resolvedContent: buildPrResolvedContent(detail, diff),
            metadata: {
              label: `#${detail.number} ${detail.title}`,
              state: resolvedState,
              fetchedAt: Date.now(),
              isLoading: false,
            },
          });
        } catch (err) {
          updateStagedAttachment(threadId, id, {
            metadata: {
              label: `#${summary.number} ${summary.title}`,
              state: initialState,
              isLoading: false,
              error: String(err),
            },
          });
        }
      })();
    },
    [threadId, cwd, addStagedAttachment, updateStagedAttachment],
  );

  /** Step 8 Stage 7 — flip the `expandFullDiff` flag on a staged PR
   *  attachment and re-resolve its content with the matching diff
   *  shape. The chip's expand affordance calls this; the resolved
   *  body the agent sees swaps from `### Files changed (N)` to
   *  `### Diff (full)` (or vice-versa). The detail fetch is cached
   *  on the Rust side so the second call after a toggle is cheap;
   *  the diff fetch with `full=true` is uncached but capped at 100KB. */
  const handleToggleExpandPr = useCallback(
    (attachmentId: string) => {
      if (!threadId) return;
      const liveSlice = useAgentChatStore.getState().threads[threadId];
      const att = liveSlice?.stagedAttachments.find((a) => a.id === attachmentId);
      if (!att || att.kind !== "pr") return;
      const num = Number.parseInt(att.ref.replace(/^!/, ""), 10);
      if (!Number.isFinite(num)) return;
      const currentLabel = att.metadata.label;
      const currentState = att.metadata.state;
      const newExpand = !att.metadata.expandFullDiff;
      updateStagedAttachment(threadId, attachmentId, {
        metadata: {
          ...att.metadata,
          expandFullDiff: newExpand,
          isLoading: true,
        },
      });
      void (async () => {
        try {
          const repoPath = cwd ?? "";
          const [detail, diff] = await Promise.all([
            getGithubPrByPath(repoPath, num),
            getGithubPrDiffByPath(repoPath, num, newExpand),
          ]);
          const detailUpper = detail.state.toUpperCase();
          const resolvedState: "open" | "closed" | "merged" | "draft" =
            detailUpper === "MERGED"
              ? "merged"
              : detailUpper === "CLOSED"
                ? "closed"
                : detail.is_draft
                  ? "draft"
                  : "open";
          updateStagedAttachment(threadId, attachmentId, {
            resolvedContent: buildPrResolvedContent(detail, diff, {
              fullDiff: newExpand,
            }),
            metadata: {
              label: `#${detail.number} ${detail.title}`,
              state: resolvedState,
              expandFullDiff: newExpand,
              fetchedAt: Date.now(),
              isLoading: false,
            },
          });
        } catch (err) {
          updateStagedAttachment(threadId, attachmentId, {
            metadata: {
              label: currentLabel,
              state: currentState,
              expandFullDiff: newExpand,
              isLoading: false,
              error: String(err),
            },
          });
        }
      })();
    },
    [threadId, cwd, updateStagedAttachment],
  );

  /** Step 8 Stage 6 — image attachment. Validates the MIME against
   *  the allowlist (png/jpeg/webp/gif), stages a chip with
   *  isLoading=true, then resolves with the decoded bytes. The
   *  resolved bytes are kept in-memory only (`resolvedImage` is
   *  excluded from persistence) so a session restart re-prompts the
   *  user to re-paste. */
  const handleAttachImage = useCallback(
    async (file: File) => {
      if (!threadId) return;
      const liveSlice = useAgentChatStore.getState().threads[threadId];
      if ((liveSlice?.stagedAttachments.length ?? 0) >= ATTACHMENT_HARD_LIMIT) {
        toast.error("Attachment limit reached", {
          description: `Remove some attachments before adding more (max ${ATTACHMENT_HARD_LIMIT}).`,
        });
        return;
      }
      const allowed = [
        "image/png",
        "image/jpeg",
        "image/webp",
        "image/gif",
      ];
      if (!allowed.includes(file.type)) {
        toast.error("Image type not supported", {
          description: `${file.type || "unknown type"} — supported: png, jpeg, webp, gif`,
        });
        return;
      }
      // Stage 7 — animated GIFs are silently rejected by Anthropic's
      // image API. Detect at attach time so the user sees a clear
      // toast instead of an opaque 400 at send time. Static GIFs
      // (single frame) pass through unchanged.
      let resolvedBuffer: ArrayBuffer | null = null;
      if (file.type === "image/gif") {
        try {
          resolvedBuffer = await file.arrayBuffer();
          if (detectAnimatedGif(resolvedBuffer)) {
            toast.error("Animated GIFs not supported", {
              description: "Save as PNG or JPEG instead.",
            });
            return;
          }
        } catch (err) {
          toast.error("Failed to read image", { description: String(err) });
          return;
        }
      }
      const id =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `att-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const label = file.name || `pasted-image-${Date.now()}.png`;
      addStagedAttachment(threadId, {
        id,
        kind: "image",
        ref: `image:${id}`,
        metadata: {
          label,
          bytes: file.size,
          isLoading: true,
        },
      });
      try {
        const buffer = resolvedBuffer ?? (await file.arrayBuffer());
        const bytes = new Uint8Array(buffer);
        updateStagedAttachment(threadId, id, {
          resolvedImage: { mime: file.type, bytes },
          metadata: {
            label,
            bytes: file.size,
            isLoading: false,
            fetchedAt: Date.now(),
          },
        });
      } catch (err) {
        updateStagedAttachment(threadId, id, {
          metadata: {
            label,
            bytes: file.size,
            isLoading: false,
            error: String(err),
          },
        });
      }
    },
    [threadId, addStagedAttachment, updateStagedAttachment],
  );

  // Clear the optimistic send flag the moment the backend
  // acknowledges the turn via Running (streaming=true in the store).
  // For the degenerate case where Running and turn_completed batch
  // into the same render — so `streaming` appears to stay false from
  // the Composer's perspective — we also clear when `activeTurnId`
  // transitions non-null (another backend-ack signal) or when the
  // next render cycle completes without streaming flipping; the
  // sync-ref flip already prevented duplicate submits so the `ref`
  // stays correct either way.
  useEffect(() => {
    if (!isSending) return;
    if (streaming || activeTurnId != null) {
      sendInFlightRef.current = false;
      setIsSending(false);
    }
  }, [isSending, streaming, activeTurnId]);

  // Interrupt mechanic: the SDK's `query.interrupt()` causes its
  // async iterator to exit. The session is functionally dead after
  // that (ClaudeAdapter.ts:2363 calls `stopSessionInternal`
  // unconditionally). The reference impl's next `sendTurn` creates
  // a brand-new SDK query transparently via
  // `ensureSessionForThread`. Our Rust adapter has no equivalent
  // auto-recreate, so we do it proactively:
  // interrupt for the immediate turn abort, then stop + start the
  // session so subsequent turns land on a live SDK query. Transcript
  // and picker state persist via `migrateThreadId`.
  const handleStop = useCallback(() => {
    if (!threadId) return;
    if (restarting) return;
    const currentSlice = useAgentChatStore.getState().threads[threadId];
    if (!currentSlice) return;
    setRestarting(true);
    void (async () => {
      // Fire-and-forget the interrupt RPC so the SDK query aborts
      // immediately — don't block on it.
      agentChatInterruptTurn(provider, threadId, null).catch(() => {
        // Stop path will also tear down the sidecar; an interrupt
        // failure here is safe to swallow.
      });
      try {
        await agentChatStopSession(provider, threadId);
      } catch (err) {
        console.warn("[agent-chat] stop_session during Stop failed", err);
      }
      if (!cwd) {
        setRestarting(false);
        return;
      }
      try {
        const newLocalThreadId = `chat-${pane.pane_id}-${Date.now()}`;
        const newId = await agentChatStartSession(
          pane.pane_id,
          provider,
          {
            thread_id: newLocalThreadId,
            cwd,
            model: currentSlice.model,
            resume_cursor: currentSlice.resumeCursor,
            permission_mode: currentSlice.permissionMode,
            effort: currentSlice.effort,
            context_window: currentSlice.contextWindow,
            additional_directories: [],
            env: null,
          },
        );
        migrateThreadId(threadId, newId);
        setThreadId(newId);
        setSessionLaunchMode(newId, currentSlice.permissionMode);
      } catch (err) {
        toast.error(`Failed to restart session after stop: ${err}`);
      } finally {
        setRestarting(false);
        // Belt-and-braces: restart cleared any in-flight state, so
        // drop the local send guard.
        sendInFlightRef.current = false;
        setIsSending(false);
      }
    })();
  }, [
    threadId,
    provider,
    cwd,
    pane.pane_id,
    restarting,
    migrateThreadId,
    setSessionLaunchMode,
  ]);

  const handleRespond = useCallback(
    (requestId: string, decision: ApprovalDecision) => {
      if (!threadId) return;
      markRequestResponding(threadId, requestId, decision);
      agentChatRespondToRequest(provider, threadId, requestId, decision).catch(
        (err) => {
          toast.error(`Failed to send decision: ${err}`);
        },
      );
    },
    [threadId, provider, markRequestResponding],
  );

  /**
   * Shared silent-restart helper. Permission-mode, effort, and
   * context-window all trigger the same flow on Claude: stop the
   * current session, start a new one with the updated launch params,
   * migrate the thread id. Transcript + model + draft survive via
   * `migrateThreadId`; resume cursor carries session state when
   * available. Codex-side callers don't route through here.
   *
   * Declared above the mode handlers so `handleModeRemove` can use
   * it for the silent-restart restore path (the SDK rejects live
   * `setPermissionMode("bypassPermissions")` even when the launch
   * carried `--dangerously-skip-permissions`).
   */
  const restartSessionWith = useCallback(
    (updates: {
      permissionMode?: string;
      effort?: string | null;
      contextWindow?: string | null;
    }) => {
      if (!threadId) return;
      const currentSlice = useAgentChatStore.getState().threads[threadId];
      if (!currentSlice) return;
      if (restarting) return;
      setRestarting(true);
      const resumeCursor = currentSlice.resumeCursor;
      const newLocalThreadId = `chat-${pane.pane_id}-${Date.now()}`;
      const nextMode = updates.permissionMode ?? currentSlice.permissionMode;
      const nextEffort =
        updates.effort !== undefined ? updates.effort : currentSlice.effort;
      const nextContext =
        updates.contextWindow !== undefined
          ? updates.contextWindow
          : currentSlice.contextWindow;
      void (async () => {
        try {
          await agentChatStopSession(provider, threadId);
        } catch (err) {
          console.warn("[agent-chat] stop_session during restart failed", err);
        }
        try {
          const newId = await agentChatStartSession(pane.pane_id, provider, {
            thread_id: newLocalThreadId,
            cwd: cwd ?? "",
            model: currentSlice.model,
            resume_cursor: resumeCursor,
            permission_mode: nextMode,
            effort: nextEffort,
            context_window: nextContext,
            additional_directories: [],
            env: null,
          });
          migrateThreadId(threadId, newId);
          setThreadId(newId);
          setSessionLaunchMode(newId, nextMode);
        } catch (err) {
          toast.error(`Failed to restart session: ${err}`);
        } finally {
          setRestarting(false);
        }
      })();
    },
    [
      threadId,
      provider,
      cwd,
      pane.pane_id,
      restarting,
      migrateThreadId,
      setSessionLaunchMode,
    ],
  );

  // Plan-accept: flip the live session to `default` permission mode
  // (Claude adapter wires `query.setPermissionMode` directly — no
  // session restart) and send a synthetic "Proceed with the plan."
  // turn so the model un-sticks from the deny + interrupt that
  // ExitPlanMode triggered. This deliberately bypasses
  // `handlePermissionModeChange`, which goes through
  // `restartSessionWith` because Claude's capability declares
  // per-session granularity. Research 1 confirmed the live setter
  // is safe.
  const handleAcceptPlan = useCallback(
    async (requestId: string) => {
      if (!threadId) return;
      // Collapse the plan card locally. The sidecar denied+interrupted
      // the ExitPlanMode tool before emitting the request, so no
      // `request-resolved` event will ever arrive — without this the
      // card stays on its pending affordances and pins the transcript
      // tail in a "user must act" state that suppresses the thinking
      // indicator during the synthetic turn that follows.
      markRequestResolved(threadId, requestId, { decision: "allow" });
      try {
        await agentChatSetPermissionMode(provider, threadId, "default");
        setStorePermissionMode(threadId, "default");
        setSessionLaunchMode(threadId, "default");
        // Stage 3: accepting a plan also clears the Plan pill so the
        // picker reappears and the composer returns to normal. The
        // stashed priorPermissionMode is discarded — the user explicitly
        // opted into `default` by accepting.
        setStoreMode(threadId, "default");
        setStoreModePriorPermissionMode(threadId, null);
        await agentChatSendTurn(provider, {
          thread_id: threadId,
          text: "Proceed with the plan.",
          model_override: null,
          effort_override: null,
          permission_mode_override: null,
        });
      } catch (err) {
        toast.error(`Failed to accept plan: ${err}`);
      }
    },
    [
      threadId,
      provider,
      markRequestResolved,
      setStorePermissionMode,
      setSessionLaunchMode,
      setStoreMode,
      setStoreModePriorPermissionMode,
    ],
  );

  // Plan-reject: send a new user turn carrying the feedback (or a
  // generic revise prompt when empty). The ExitPlanMode canUseTool
  // was already denied + interrupted by the sidecar before this
  // card rendered, so there is no approval to resolve here — just a
  // fresh turn that lands on the still-open plan-mode session.
  const handleRejectPlan = useCallback(
    async (requestId: string) => {
      if (!threadId) return;
      // Same reasoning as handleAcceptPlan: no sidecar round-trip, so
      // collapse the card locally before firing the follow-up turn.
      markRequestResolved(threadId, requestId, {
        decision: "deny",
        message: "Please revise the plan.",
      });
      try {
        await agentChatSendTurn(provider, {
          thread_id: threadId,
          text: "Please revise the plan.",
          model_override: null,
          effort_override: null,
          permission_mode_override: null,
        });
      } catch (err) {
        toast.error(`Failed to reject plan: ${err}`);
      }
    },
    [threadId, provider, markRequestResolved],
  );

  // Composer-level mode pill activation. Plan and Ask both flip the
  // live session to `permission_mode: "plan"` via `setPermissionMode`
  // (no restart per Research 1) — Plan because the SDK enforces the
  // read-only contract via the picker semantics, Ask because we want
  // the same SDK-level write enforcement under the hood (the per-turn
  // prompt wrapper handled in `applyAllPrefixes` tells the model to
  // answer conversationally instead of calling ExitPlanMode). The
  // prior picker value is stashed so toggle-off can restore it.
  // Debug doesn't override permission_mode — it's prompt-wrapper-only
  // enforcement, so the slice flip is the entire activation.
  const handleModeActivate = useCallback(
    async (newMode: ActivePillMode) => {
      if (!threadId) return;
      const currentSlice = useAgentChatStore.getState().threads[threadId];
      if (!currentSlice) return;
      if (currentSlice.mode === newMode) return;
      if (newMode === "plan" || newMode === "ask") {
        const priorMode = currentSlice.permissionMode;
        try {
          await agentChatSetPermissionMode(provider, threadId, "plan");
        } catch (err) {
          const label = newMode === "plan" ? "Plan" : "Ask";
          toast.error(`Failed to activate ${label} mode: ${err}`);
          return;
        }
        setStoreModePriorPermissionMode(threadId, priorMode);
        setStorePermissionMode(threadId, "plan");
        setSessionLaunchMode(threadId, "plan");
        setStoreMode(threadId, newMode);
      } else {
        setStoreMode(threadId, newMode);
      }
    },
    [
      threadId,
      provider,
      setStoreModePriorPermissionMode,
      setStorePermissionMode,
      setSessionLaunchMode,
      setStoreMode,
    ],
  );

  /** Synthetic cleanup turn. Switches the slice off Debug first so
   *  the wrapper doesn't re-instruct the model to add markers, then
   *  asks the model to grep+remove every CODEMUX_DEBUG line and any
   *  scaffolding that supported them.
   *
   *  Optimistic-clear caveat: `hasDebugActivity` flips to false the
   *  moment `agentChatSendTurn` resolves (which is RPC-accept, not
   *  turn-completion). If Claude leaves stragglers, the banner
   *  disappears even though markers remain. The next mount-effect
   *  re-grep on chat-open reconciles this — so the stale state is
   *  bounded to the current pane lifetime.
   *
   *  Deferred: a post-`turn_completed` verification grep would close
   *  this gap inside the current session. Wiring it requires a
   *  one-shot listener tied to the turn id returned from
   *  `agentChatSendTurn` (events flow through `useAgentChatEvents`
   *  → `applyEvent`; nothing currently subscribes per-turn). The
   *  marginal value is low — Claude's cleanup is reliable in
   *  practice and the user can always click Clean up again — so
   *  this stays out of Stage 6. */
  const triggerDebugCleanup = useCallback(async () => {
    if (!threadId) return;
    if (cleanupInFlightRef.current) return;
    cleanupInFlightRef.current = true;
    setCleanupInFlight(true);
    setStoreMode(threadId, "default");
    try {
      await agentChatSendTurn(provider, {
        thread_id: threadId,
        text: CLEANUP_PROMPT,
        model_override: null,
        effort_override: null,
        permission_mode_override: null,
      });
      setStoreHasDebugActivity(threadId, false);
    } catch (err) {
      toast.error(`Failed to start debug cleanup: ${err}`);
    } finally {
      cleanupInFlightRef.current = false;
      setCleanupInFlight(false);
    }
  }, [threadId, provider, setStoreMode, setStoreHasDebugActivity]);

  /** Open the exit-confirmation dialog and resolve with the user's
   *  pick. Caller awaits the choice and dispatches accordingly. The
   *  dialog itself unmounts the moment a choice resolves. */
  const confirmDebugExit = useCallback((): Promise<DebugExitChoice> => {
    return new Promise((resolve) => {
      setDebugExitDialog({
        resolve: (choice) => {
          setDebugExitDialog(null);
          resolve(choice);
        },
      });
    });
  }, []);

  // Composer-level mode pill removal. Plan and Ask both restore the
  // stashed prior `permissionMode` via a silent session restart
  // rather than the live `setPermissionMode` setter — the SDK only
  // honours `--dangerously-skip-permissions` at launch time, so a
  // live switch BACK to `bypassPermissions` is rejected even though
  // the session was originally launched with the flag. Restarting
  // with the prior mode in launch params re-applies the flag
  // correctly. Transcript + pickers carry across via
  // `migrateThreadId`. Debug pops the cleanup-or-leave dialog when
  // markers exist; otherwise it just flips the slice back to default.
  // Missing prior falls back to the provider's default mode so the
  // session is never left in an orphan state.
  const handleModeRemove = useCallback(async () => {
    if (!threadId) return;
    const currentSlice = useAgentChatStore.getState().threads[threadId];
    if (!currentSlice) return;
    if (currentSlice.mode === "plan" || currentSlice.mode === "ask") {
      const restore =
        currentSlice.modePriorPermissionMode ??
        DEFAULT_THREAD_PERMISSION_MODE;
      // Snap the slice for immediate UI feedback (picker reappears,
      // pill drops). The restart sets sessionLaunchMode itself.
      setStorePermissionMode(threadId, restore);
      restartSessionWith({ permissionMode: restore });
      setStoreMode(threadId, "default");
      setStoreModePriorPermissionMode(threadId, null);
      return;
    }
    if (currentSlice.mode === "debug") {
      if (
        currentSlice.hasDebugActivity &&
        currentSlice.debugActivityResolved
      ) {
        const choice = await confirmDebugExit();
        if (choice === "cancel") return;
        if (choice === "cleanup") {
          await triggerDebugCleanup();
          return;
        }
        // "leave" — fall through to the default pill-drop below.
      }
      setStoreMode(threadId, "default");
      return;
    }
    setStoreMode(threadId, "default");
    setStoreModePriorPermissionMode(threadId, null);
  }, [
    threadId,
    setStorePermissionMode,
    setStoreMode,
    setStoreModePriorPermissionMode,
    restartSessionWith,
    confirmDebugExit,
    triggerDebugCleanup,
  ]);

  const handleModelChange = useCallback(
    (next: string) => {
      if (!threadId) return;
      setStoreModel(threadId, next);
      // Compatibility rule — use a pure planner so the decision is
      // testable in isolation (see `planModelChange`). Reads from the
      // fallback capability snapshot so the reset is correct even
      // while live data is still loading.
      const nextModel =
        capabilities?.models.find((m) => m.id === next) ?? null;
      const plan = planModelChange({
        newModel: nextModel,
        currentEffort: effort,
        currentContextWindow: contextWindow,
      });
      if (plan.resetEffort !== undefined) {
        setStoreEffort(threadId, plan.resetEffort);
      }
      if (plan.resetContextWindow !== undefined) {
        setStoreContextWindow(threadId, plan.resetContextWindow);
      }
      agentChatSetModel(provider, threadId, next).catch((err) => {
        toast.error(`Failed to set model: ${err}`);
      });
    },
    [
      threadId,
      provider,
      capabilities,
      effort,
      contextWindow,
      setStoreModel,
      setStoreEffort,
      setStoreContextWindow,
    ],
  );

  const handlePermissionModeChange = useCallback(
    (next: string) => {
      if (!threadId) return;
      const currentSlice = useAgentChatStore.getState().threads[threadId];
      if (!currentSlice) return;
      // Delegate the decision to `planPermissionModeChange` — it
      // checks the mode is valid for the active provider and reads
      // `permission_granularity` from capabilities to decide whether
      // to restart. Returns null for unknown modes.
      const plan = planPermissionModeChange({
        nextMode: next,
        capabilities,
      });
      if (!plan) return;
      setStorePermissionMode(threadId, plan.setPermissionMode);
      // Skip the restart when the same mode is already live on the
      // current session — avoids a no-op session teardown.
      if (currentSlice.sessionLaunchMode === plan.setPermissionMode) return;
      if (plan.restart) {
        restartSessionWith({ permissionMode: plan.setPermissionMode });
      }
      // PerTurn providers: the mode is already persisted; the next
      // `sendTurn` picks it up via `permission_mode_override`.
    },
    [threadId, capabilities, setStorePermissionMode, restartSessionWith],
  );

  /**
   * Effort change — delegates the decision to `planEffortChange` so
   * the three-branch logic (ultrathink prepend / strip-and-set / plain
   * set) can be unit-tested in isolation. This handler is pure
   * action-dispatch.
   */
  const handleEffortChange = useCallback(
    (next: string) => {
      if (!threadId) return;
      const plan = planEffortChange({
        nextEffort: next,
        model: activeModel,
        currentDraft: draft,
        provider,
      });
      if (!plan) return;
      if (plan.updateDraft) {
        setInputDraft(threadId, plan.updateDraft.nextDraft);
      }
      if (plan.setEffort !== null) {
        setStoreEffort(threadId, plan.setEffort);
      }
      if (plan.restart) {
        restartSessionWith({ effort: plan.setEffort });
      }
    },
    [
      threadId,
      activeModel,
      draft,
      provider,
      setInputDraft,
      setStoreEffort,
      restartSessionWith,
    ],
  );

  const handleContextWindowChange = useCallback(
    (next: string) => {
      if (!threadId) return;
      setStoreContextWindow(threadId, next);
      // Context window on Claude is encoded into the model id (e.g.
      // `claude-opus-4-7[1m]`), which is a session-init parameter.
      // Mid-session change → restart.
      if (provider === "claude") {
        restartSessionWith({ contextWindow: next });
      }
    },
    [threadId, provider, setStoreContextWindow, restartSessionWith],
  );

  const handleProviderChange = useCallback(
    (next: AgentChatProviderKind) => {
      // Step 2: provider swap is not safe once a session exists (the
      // thread is bound to a single adapter). Hide the picker by
      // default and fall back to a toast if it ever fires.
      if (threadId) {
        toast.warning("Provider cannot be changed after a session starts.");
        return;
      }
      setProvider(next);
    },
    [threadId],
  );

  const sessionReady = threadId != null && !starting && !restarting;

  // Derivative branch — base the "+ New worktree…" inline submit
  // forks from. Persists for the pane's lifetime; defaults to "main".
  const [derivativeBranch, setDerivativeBranch] = useState("main");

  // Zone 1 dispatch — Home-rooted panes get a ProjectPicker so the
  // user can hop to a different project mid-conversation. All other
  // panes get a WorktreePicker + DerivativeBranchPicker pair scoped to
  // this pane's project so the user can switch worktrees or create
  // one inline.
  const zone1Override = (() => {
    if (!workspaceProjectRoot) return null;
    if (isHomeWorkspace) {
      return (
        <ProjectPicker
          value={null}
          onChange={(targetProjectPath) => {
            // Clear any active draft — the project switch promotes us
            // into a real workspace, so the draft surface should not
            // re-mount on top of it (Stage C Bug-2 pattern).
            useChatDraftStore.getState().setActiveDraft(null);
            const snapshot = useAppStore.getState().appState;
            const groups = snapshot
              ? groupWorkspacesByProject(snapshot.workspaces, homeDir)
              : [];
            const targetGroup = groups.find(
              (g) => g.projectPath === targetProjectPath,
            );
            const target = targetGroup?.workspaces[0];
            if (target) {
              activateWorkspace(target.workspace_id).catch(console.error);
            } else {
              setShowNewWorkspaceDialog(true, targetProjectPath);
            }
          }}
        />
      );
    }
    return (
      <div className="flex items-center gap-2">
        <WorktreePicker
          mode="active"
          projectPath={workspaceProjectRoot}
          currentWorkspaceId={workspaceIdForPane ?? undefined}
          derivativeBranch={derivativeBranch}
          onSwitchWorkspace={(wsId) => {
            // Bug-2 draft-clear pattern: any draft pinned to this slot
            // would otherwise re-render on top of the activated
            // workspace's pane.
            useChatDraftStore.getState().setActiveDraft(null);
            activateWorkspace(wsId).catch(console.error);
          }}
          onWorktreeCreated={async (wsId) => {
            // Pre-start the session before activating — otherwise
            // the newly mounted AgentChatPane races to mint its own
            // thread_id, and the user's first send can land before
            // the session is registered in the adapter's HashMap
            // (→ `session_not_found`). See
            // `prestart-worktree-session.ts` for the rationale.
            try {
              await prestartWorktreeSession(wsId);
            } catch (err) {
              console.error(
                "Failed to prestart worktree chat session:",
                err,
              );
            }
            useChatDraftStore.getState().setActiveDraft(null);
            activateWorkspace(wsId).catch(console.error);
          }}
        />
        <DerivativeBranchPicker
          projectPath={workspaceProjectRoot}
          value={derivativeBranch}
          onChange={setDerivativeBranch}
        />
      </div>
    );
  })();

  // AskUserQuestion prompts render as a composer-attached panel
  // (one-question-per-card pattern, similar to Claude.ai) rather
  // than inline in the transcript. Only the first pending
  // user-input request surfaces; MessageList
  // reduces user-input items to a tiny marker so the transcript
  // doesn't duplicate the prompt.
  const pendingUserInput = useMemo<ChatViewItem | null>(() => {
    for (const m of messages) {
      if (
        m.kind === "permission_request" &&
        m.request_kind === "user-input" &&
        m.resolution.state === "pending"
      ) {
        return m;
      }
    }
    return null;
  }, [messages]);

  const handleSubmitUserInput = useCallback(
    (output: AskUserQuestionOutput) => {
      if (!pendingUserInput || pendingUserInput.kind !== "permission_request") {
        return;
      }
      handleRespond(pendingUserInput.request_id, {
        decision: "allow",
        updated_input: output,
      });
    },
    [pendingUserInput, handleRespond],
  );

  const pendingInputPanelEl =
    pendingUserInput && pendingUserInput.kind === "permission_request" ? (
      <ComposerPendingInputPanel
        // Remount when the pending request_id changes so per-question
        // state (picks / free-text / current index) resets for a new
        // prompt instead of leaking from the previous one.
        key={pendingUserInput.request_id}
        item={pendingUserInput}
        onSubmit={handleSubmitUserInput}
      />
    ) : null;

  const composerEl = (
    <Composer
      draft={draft}
      cwd={cwd}
      zone1Override={zone1Override}
      provider={provider}
      model={model}
      permissionMode={permissionMode}
      effort={effort}
      contextWindow={contextWindow}
      activeModel={activeModel}
      effortLabelMap={effortLabelMap}
      permissionModes={permissionModes}
      ultrathinkInBodyText={ultrathinkInBodyText}
      streaming={streaming || isSending}
      sessionReady={sessionReady}
      showProviderPicker={ENABLE_PROVIDER_PICKER}
      mode={mode}
      stagedAttachments={stagedAttachments}
      onRemoveAttachment={(id) => {
        if (!threadId) return;
        removeStagedAttachment(threadId, id);
      }}
      onToggleExpandPr={handleToggleExpandPr}
      onAttachFile={handleAttachFile}
      onAttachFolder={handleAttachFolder}
      onAttachIssue={handleAttachIssue}
      onAttachPr={handleAttachPr}
      onAttachImage={handleAttachImage}
      modelSupportsImages={activeModel?.supports_images ?? false}
      isGithubRepo={isGithubRepo}
      ghAuthenticated={ghAuthenticated}
      onDraftChange={(next) => {
        if (!threadId) return;
        setInputDraft(threadId, next);
      }}
      onSubmit={handleSubmit}
      onStop={handleStop}
      onProviderChange={handleProviderChange}
      onModelChange={handleModelChange}
      onPermissionModeChange={handlePermissionModeChange}
      onEffortChange={handleEffortChange}
      onContextWindowChange={handleContextWindowChange}
      onModeActivate={handleModeActivate}
      onModeRemove={handleModeRemove}
    />
  );

  const debugBannerEl =
    mode === "debug" && hasDebugActivity && debugActivityResolved ? (
      <DebugCleanupBanner
        onCleanup={triggerDebugCleanup}
        busy={cleanupInFlight}
      />
    ) : null;

  return (
    <div className="flex h-full w-full flex-col bg-background">
      {messages.length === 0 ? (
        <ChatHomeLanding composer={composerEl} />
      ) : (
        <>
          <ChatTranscript
            messages={messages}
            streaming={streaming || isSending}
            onRespondToRequest={handleRespond}
            onAcceptPlan={handleAcceptPlan}
            onRejectPlan={handleRejectPlan}
          />
          {pendingInputPanelEl}
          {debugBannerEl}
          {composerEl}
        </>
      )}
      <DebugExitDialog
        open={debugExitDialog !== null}
        onChoose={(choice) => debugExitDialog?.resolve(choice)}
      />
    </div>
  );
}

const EMPTY_MESSAGES: ChatViewItem[] = [];
const EMPTY_ATTACHMENTS: Attachment[] = [];
