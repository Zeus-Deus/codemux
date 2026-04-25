import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { useAgentChatEvents } from "@/hooks/use-agent-chat-events";
import {
  planCapabilityCompatReset,
  planEffortChange,
  planModelChange,
  planPermissionModeChange,
  planSubmit,
} from "@/lib/agent-chat/chat-pane-plans";
import { applyAllPrefixes } from "@/lib/agent-chat/mode-prefix";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import { hasUltrathinkInBodyText } from "@/lib/agent-chat/ultrathink";
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
} from "@/stores/agent-chat-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import {
  selectCapabilities,
  selectModel,
  useProviderCapabilities,
} from "@/stores/provider-capabilities-store";
import {
  activateWorkspace,
  agentChatInterruptTurn,
  agentChatRespondToRequest,
  agentChatSendTurn,
  agentChatSetModel,
  agentChatSetPermissionMode,
  agentChatStartSession,
  agentChatStopSession,
} from "@/tauri/commands";
import { prestartWorktreeSession } from "@/lib/agent-chat/prestart-worktree-session";
import type { AgentChatEventPayload, ApprovalDecision } from "@/tauri/events";
import type {
  AgentChatProviderKind,
  PaneNodeSnapshot,
} from "@/tauri/types";

import { ChatTranscript } from "./ChatTranscript";
import { ChatHomeLanding } from "./ChatHomeLanding";
import { Composer } from "./Composer";
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

// Codex's set_permission_mode currently rejects unknown strings. Hide
// the provider picker until the backend exposes a capability probe.
const ENABLE_PROVIDER_PICKER = false;

export function AgentChatPane({ pane }: { pane: AgentChatPaneNode }) {
  const initialProvider: AgentChatProviderKind = pane.provider ?? "claude";
  const [provider, setProvider] = useState<AgentChatProviderKind>(initialProvider);
  const [threadId, setThreadId] = useState<string | null>(pane.thread_id);
  const [starting, setStarting] = useState(false);
  const [restarting, setRestarting] = useState(false);
  // Optimistic in-flight flag mirroring T3Code's `isSendBusy`
  // (ChatView.tsx:406). Set synchronously on submit so the button
  // disables BEFORE the backend's Running event round-trips.
  //
  // NOTE: `isSending` is useState (drives the render), but rapid-fire
  // Enter presses in the SAME JS tick don't see the updated state —
  // the `useCallback` closure captured the pre-set snapshot. T3Code
  // pairs the state with a `sendInFlightRef.current` synchronous
  // guard (ChatView.tsx:2458) for exactly this reason: refs mutate
  // synchronously, so the second call within the same tick sees the
  // flag the first one just set. We do the same.
  const [isSending, setIsSending] = useState(false);
  const sendInFlightRef = useRef(false);

  const fallbackCwd = useAppStore((s) => {
    if (!s.appState) return null;
    const ws = s.appState.workspaces.find(
      (w) => w.workspace_id === s.appState!.active_workspace_id,
    );
    return ws?.cwd ?? null;
  });
  const cwd = pane.cwd ?? fallbackCwd;

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
    const match = Object.values(s.draftsById).find(
      (d) => d.promotedTo?.workspaceId === workspaceIdForPane,
    );
    return match?.threadId ?? null;
  });
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
  const setStoreEffort = useAgentChatStore((s) => s.setEffort);
  const setStoreContextWindow = useAgentChatStore((s) => s.setContextWindow);
  const setStoreMode = useAgentChatStore((s) => s.setMode);
  const setStoreModePriorPermissionMode = useAgentChatStore(
    (s) => s.setModePriorPermissionMode,
  );
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

  // Subscribe to provider events for this thread. The handler reads
  // store actions via `getState()` so it stays stable across
  // re-renders — otherwise we'd rebind the Tauri listener on every
  // keystroke.
  const handleEvent = useCallback((payload: AgentChatEventPayload) => {
    useAgentChatStore.getState().applyEvent(payload.thread_id, payload.event);
  }, []);
  useAgentChatEvents(threadId, handleEvent);

  // Start a session on mount if the pane doesn't already have one.
  const startAttempted = useRef(false);
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
    starting,
    pane.pane_id,
    provider,
    cwd,
    ensureThread,
    setStoreModel,
    setStorePermissionMode,
    setSessionLaunchMode,
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
    // Mode wrappers (Stage 4 onward) live SDK-side only — the
    // transcript stores the unwrapped (ultrathink-only) text so users
    // see what they typed, not the framing we layered on top.
    const sdkText = applyAllPrefixes(rawText, mode, effort);
    sendInFlightRef.current = true;
    setIsSending(true);
    appendUserMessage(threadId, plan.text);
    const input = {
      thread_id: threadId,
      text: sdkText,
      model_override: null,
      effort_override: plan.effortOverride,
    };
    agentChatSendTurn(provider, input).catch((err) => {
      toast.error(`Failed to send turn: ${err}`);
      sendInFlightRef.current = false;
      setIsSending(false);
    });
  }, [threadId, draft, provider, effort, mode, appendUserMessage]);

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

  // T3Code-aligned interrupt: the SDK's `query.interrupt()` causes its
  // async iterator to exit. The session is functionally dead after
  // that (ClaudeAdapter.ts:2363 calls `stopSessionInternal`
  // unconditionally). T3Code's next `sendTurn` creates a brand-new
  // SDK query transparently via `ensureSessionForThread`. Our Rust
  // adapter has no equivalent auto-recreate, so we do it proactively:
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
  // Debug lands in Stage 6 — currently no-ops past the slice update.
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
        // Debug: state-only flip until Stage 6 wires its wrapper.
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

  // Composer-level mode pill removal. Plan and Ask both restore the
  // stashed prior `permissionMode` via a silent session restart
  // rather than the live `setPermissionMode` setter — the SDK only
  // honours `--dangerously-skip-permissions` at launch time, so a
  // live switch BACK to `bypassPermissions` is rejected even though
  // the session was originally launched with the flag. Restarting
  // with the prior mode in launch params re-applies the flag
  // correctly. Transcript + pickers carry across via
  // `migrateThreadId`. Debug just flips the slice back to `default`
  // until Stage 6. Missing prior falls back to the provider's
  // default mode so the session is never left in an orphan state.
  const handleModeRemove = useCallback(() => {
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
    }
    setStoreMode(threadId, "default");
    setStoreModePriorPermissionMode(threadId, null);
  }, [
    threadId,
    setStorePermissionMode,
    setStoreMode,
    setStoreModePriorPermissionMode,
    restartSessionWith,
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
  // (t3code / Claude.ai pattern) rather than inline in the transcript.
  // Only the first pending user-input request surfaces; MessageList
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
          {composerEl}
        </>
      )}
    </div>
  );
}

const EMPTY_MESSAGES: ChatViewItem[] = [];
