import { useCallback, useEffect, useRef, useState } from "react";

import { useAgentChatEvents } from "@/hooks/use-agent-chat-events";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import { toast } from "@/lib/toast";
import { useAppStore } from "@/stores/app-store";
import {
  DEFAULT_THREAD_PERMISSION_MODE,
  useAgentChatStore,
} from "@/stores/agent-chat-store";
import {
  agentChatInterruptTurn,
  agentChatRespondToRequest,
  agentChatSendTurn,
  agentChatSetModel,
  agentChatStartSession,
  agentChatStopSession,
} from "@/tauri/commands";
import type { AgentChatEventPayload, ApprovalDecision } from "@/tauri/events";
import type {
  AgentChatProviderKind,
  PaneNodeSnapshot,
} from "@/tauri/types";

import { ChatTranscript } from "./ChatTranscript";
import { Composer } from "./Composer";
import { defaultModelForProvider } from "./pickers/ModelPicker";

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
  // disables BEFORE the backend's Running event round-trips. Without
  // this, a fast second submit races the event and the backend rejects
  // with "session has an active turn".
  const [isSending, setIsSending] = useState(false);

  const fallbackCwd = useAppStore((s) => {
    if (!s.appState) return null;
    const ws = s.appState.workspaces.find(
      (w) => w.workspace_id === s.appState!.active_workspace_id,
    );
    return ws?.cwd ?? null;
  });
  const cwd = pane.cwd ?? fallbackCwd;

  const ensureThread = useAgentChatStore((s) => s.ensureThread);
  const setInputDraft = useAgentChatStore((s) => s.setInputDraft);
  const setStoreModel = useAgentChatStore((s) => s.setModel);
  const setStorePermissionMode = useAgentChatStore((s) => s.setPermissionMode);
  const setSessionLaunchMode = useAgentChatStore(
    (s) => s.setSessionLaunchMode,
  );
  const migrateThreadId = useAgentChatStore((s) => s.migrateThreadId);
  const appendUserMessage = useAgentChatStore((s) => s.appendUserMessage);
  const markRequestResponding = useAgentChatStore(
    (s) => s.markRequestResponding,
  );

  const slice = useAgentChatStore((s) =>
    threadId ? s.threads[threadId] : null,
  );
  const draft = slice?.inputDraft ?? "";
  const messages = slice?.messages ?? EMPTY_MESSAGES;
  const streaming = slice?.streaming ?? false;
  const model = slice?.model ?? null;
  const permissionMode =
    slice?.permissionMode ?? DEFAULT_THREAD_PERMISSION_MODE;

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
    agentChatStartSession(pane.pane_id, provider, startInput)
      .then((id) => {
        setThreadId(id);
        ensureThread(id);
        setStoreModel(id, defaultModelForProvider(provider));
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
    if (!threadId) return;
    if (isSending) return;
    const text = draft.trim();
    if (!text) return;
    setIsSending(true);
    appendUserMessage(threadId, text);
    const input = {
      thread_id: threadId,
      text,
      model_override: null,
    };
    agentChatSendTurn(provider, input).catch((err) => {
      toast.error(`Failed to send turn: ${err}`);
      setIsSending(false);
    });
  }, [threadId, isSending, draft, provider, appendUserMessage]);

  // Clear the optimistic send flag once the backend acknowledges the
  // turn (Running event → streaming=true in the store) OR once the
  // turn finishes (streaming back to false). Either transition means
  // the backend now owns the authoritative turn state and the local
  // guard is no longer needed.
  useEffect(() => {
    if (isSending && streaming) setIsSending(false);
  }, [isSending, streaming]);

  const handleStop = useCallback(() => {
    if (!threadId) return;
    agentChatInterruptTurn(provider, threadId, null).catch((err) => {
      toast.error(`Failed to stop turn: ${err}`);
    });
  }, [threadId, provider]);

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

  const handleModelChange = useCallback(
    (next: string) => {
      if (!threadId) return;
      setStoreModel(threadId, next);
      agentChatSetModel(provider, threadId, next).catch((err) => {
        toast.error(`Failed to set model: ${err}`);
      });
    },
    [threadId, provider, setStoreModel],
  );

  const handlePermissionModeChange = useCallback(
    (next: string) => {
      if (!threadId) return;
      const currentSlice = useAgentChatStore.getState().threads[threadId];
      if (!currentSlice) return;
      // Persist the user's choice immediately so the picker label
      // updates regardless of restart outcome.
      setStorePermissionMode(threadId, next);
      // T3Code-aligned: mode swap ≡ session restart. The SDK only
      // respects permission-mode at launch (and `bypassPermissions`
      // additionally requires `allowDangerouslySkipPermissions` at
      // launch — there is no mid-session escalation path). Rather
      // than introducing two restart flows, restart on every change.
      if (currentSlice.sessionLaunchMode === next) return;
      if (restarting) return;
      setRestarting(true);
      const resumeCursor = currentSlice.resumeCursor;
      const newLocalThreadId = `chat-${pane.pane_id}-${Date.now()}`;
      void (async () => {
        try {
          await agentChatStopSession(provider, threadId);
        } catch (err) {
          // stop_session is idempotent on the provider; log and
          // continue so a transient stop failure doesn't strand the
          // user on a misconfigured session.
          console.warn("[agent-chat] stop_session during restart failed", err);
        }
        try {
          const newId = await agentChatStartSession(
            pane.pane_id,
            provider,
            {
              thread_id: newLocalThreadId,
              cwd: cwd ?? "",
              model: currentSlice.model,
              resume_cursor: resumeCursor,
              permission_mode: next,
              additional_directories: [],
              env: null,
            },
          );
          migrateThreadId(threadId, newId);
          setThreadId(newId);
          setSessionLaunchMode(newId, next);
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
      setStorePermissionMode,
      migrateThreadId,
      setSessionLaunchMode,
    ],
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

  return (
    <div className="flex h-full w-full flex-col bg-background">
      <ChatTranscript
        messages={messages}
        onRespondToRequest={handleRespond}
      />
      <Composer
        draft={draft}
        cwd={cwd}
        provider={provider}
        model={model}
        permissionMode={permissionMode}
        streaming={streaming || isSending}
        sessionReady={sessionReady}
        showProviderPicker={ENABLE_PROVIDER_PICKER}
        onDraftChange={(next) => {
          if (!threadId) return;
          setInputDraft(threadId, next);
        }}
        onSubmit={handleSubmit}
        onStop={handleStop}
        onProviderChange={handleProviderChange}
        onModelChange={handleModelChange}
        onPermissionModeChange={handlePermissionModeChange}
      />
    </div>
  );
}

const EMPTY_MESSAGES: ChatViewItem[] = [];
