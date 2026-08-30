import { useCallback, useState } from "react";

import { defaultModelForProvider } from "@/components/chat/pickers/ModelPicker";
import {
  launchAdoptedThread,
  launchResumedRecord,
} from "@/lib/agent-chat/adopt-external-session";
import { defaultPermissionModeForProvider } from "@/lib/agent-chat/capability-defaults";
import { toExternalAgentSession } from "@/lib/agent-chat/external-sessions";
import { toast } from "@/lib/toast";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useAppStore } from "@/stores/app-store";
import {
  agentChatAdoptExternalSession,
  agentChatGetSession,
  agentChatStartSession,
  agentChatStopSession,
  type AdoptableAgentSession,
  type AdoptExternalSessionResult,
  type AgentChatSessionRecord,
} from "@/tauri/commands";
import type {
  AgentChatProviderKind,
  PaneNodeSnapshot,
} from "@/tauri/types";

type AgentChatPaneNode = Extract<PaneNodeSnapshot, { kind: "agent_chat" }>;

/** A `/resume` pick that would run the pane in ANOTHER project's
 *  directory, parked until the user says so (R4). Rendered as a
 *  confirmation by whoever owns the pane's chrome. */
export interface ForeignProjectAdoptPrompt {
  /** The picked conversation, for its title. */
  session: AdoptableAgentSession;
  /** Directory the pane would be re-pointed at. */
  cwd: string;
}

/** Where a resume actually runs.
 *
 *  A thread's directory and the directory of the pane running it may
 *  never diverge — the history dropdown, the pane header and `@file`
 *  completion all resolve against the pane's cwd — so the two always
 *  travel together. Usually this is just the hook's own pane and its
 *  cwd; a `/resume` pick that lives in another folder is handed a pane
 *  rooted THERE by the backend. */
interface ResumeTarget {
  /** Pane that will host the session. Local thread ids and the store
   *  bookkeeping are keyed to it, not to whichever pane asked. */
  paneId: string;
  /** Directory the session launches in. `agent_chat_start_session`
   *  persists this onto the row, so it MUST be the thread's own
   *  directory — passing the pane's would silently re-point the thread. */
  cwd: string | null;
}

interface PendingForeignAdopt extends ForeignProjectAdoptPrompt {
  /** Set only when the backend had already minted the thread before we
   *  learned the conversation was foreign (the discovery row disagreed
   *  with the adopt result). Confirming then resumes from THAT result
   *  rather than adopting the same conversation a second time. */
  adopted: AdoptExternalSessionResult | null;
}

export interface AgentChatSessionActions {
  /** Resolved working directory (pane cwd, or the active workspace's cwd). */
  cwd: string | null;
  /** Provider driving the pane (defaults to `claude`). */
  provider: AgentChatProviderKind;
  /** Resume a persisted session: stop the current session, hydrate the
   *  new local thread with the picked chat's transcript, then start the
   *  provider with the record's `sdk_session_id` as the resume cursor.
   *  The record's persisted picker config (model / effort / context /
   *  permission mode) rides through to the launch so the resumed session
   *  runs in the same mode the user last chose — a NULL `permission_mode`
   *  heals to the provider default (matching what the footer pill shows
   *  for NULL rows).
   *
   *  Launches in the RECORD's directory, never the pane's: the start
   *  persists its cwd back onto the row, so resuming against the pane's
   *  would silently move the thread. */
  handleSelect: (record: AgentChatSessionRecord) => Promise<void>;
  /** Adopt a conversation the provider's own CLI created OUTSIDE
   *  Codemux (`/resume`), then run it on this pane.
   *
   *  Differs from {@link handleSelect} in three ways that matter:
   *  the thread does not exist yet (the backend mints it and binds it
   *  to the session's OWN directory — no worktree is ever created), the
   *  external session's permission mode is deliberately NOT restored
   *  (the pane's current mode wins), and there is no local history to
   *  hydrate beyond the "resumed outside Codemux" divider the backend
   *  writes. A session Codemux already owns switches to that thread
   *  instead of being adopted twice.
   *
   *  The session does not always run on THIS pane. A thread and the
   *  pane running it may never disagree about their directory, so when
   *  the conversation lives in another folder the backend opens a chat
   *  pane rooted there and returns it as `pane_id`; the launch follows
   *  it, and this pane keeps the conversation it already had.
   *
   *  A conversation from an UNRELATED project never adopts on the
   *  click: it parks in {@link AgentChatSessionActions.foreignProjectPrompt}
   *  and waits for an explicit confirm, because running it re-points
   *  the pane (and the thread bound to it) at that project's
   *  directory. Same-repo adoption stays a single click. */
  handleAdoptExternalSession: (
    session: AdoptableAgentSession,
  ) => Promise<void>;
  /** Non-null while a foreign-project `/resume` pick waits for the
   *  user's explicit yes. */
  foreignProjectPrompt: ForeignProjectAdoptPrompt | null;
  /** Proceed with the parked foreign-project adoption. */
  confirmForeignProjectAdopt: () => Promise<void>;
  /** Drop the parked foreign-project adoption; the pane stays put. */
  dismissForeignProjectAdopt: () => void;
  /** Stop the current session and start a fresh one on the same pane. */
  handleNewChat: () => Promise<void>;
}

/**
 * Shared session-switch orchestration for an agent-chat pane. Extracted
 * from {@link AgentChatPaneHeader} so the per-pane header (split layouts)
 * and the merged title-bar chat tab (GUI chrome) drive resume / new-chat
 * through ONE implementation instead of forking the SDK wiring.
 *
 * The side effect (stop current → hydrate → start with `resume`) lives
 * here because the pane snapshot already carries every input
 * `agent_chat_start_session` needs (`provider`, `cwd`, `pane_id`,
 * `thread_id`). Callers just render a trigger + list around it.
 */
export function useAgentChatSessionActions(
  pane: AgentChatPaneNode,
): AgentChatSessionActions {
  const provider: AgentChatProviderKind = pane.provider ?? "claude";

  // Fall back to the active workspace's cwd when the pane snapshot
  // hasn't stamped its own (Home pane, pre-hydrate race).
  const fallbackCwd = useAppStore((s) => {
    if (!s.appState) return null;
    const active = s.appState.active_workspace_id;
    const ws = s.appState.workspaces.find((w) => w.workspace_id === active);
    return ws?.cwd ?? null;
  });
  const cwd = pane.cwd ?? fallbackCwd;

  const paneId = pane.pane_id;
  const threadId = pane.thread_id;

  // Foreign-project `/resume` picks wait here for an explicit yes.
  const [foreignPrompt, setForeignPrompt] =
    useState<PendingForeignAdopt | null>(null);

  // Resume a persisted row on a named pane. Split out of
  // {@link handleSelect} so a `/resume` switch to a conversation that
  // lives in another folder can run on the pane rooted THERE, with the
  // row's own cwd, instead of dragging the thread onto this pane's
  // directory.
  const resumeRecord = useCallback(
    async (record: AgentChatSessionRecord, target: ResumeTarget) => {
      if (!target.cwd) {
        toast.error("Cannot resume: no working directory.");
        return;
      }
      const sdkSessionId = record.sdk_session_id;
      if (!sdkSessionId) {
        toast.warning(
          "This chat hasn't finished its first turn yet — can't resume.",
        );
        return;
      }
      // Only the pane this hook owns is ours to interrupt. When the
      // resume runs elsewhere that pane's own tab is what changes, and
      // this pane keeps the conversation the user left in it.
      const onThisPane = target.paneId === paneId;
      try {
        if (onThisPane && threadId) {
          await agentChatStopSession(provider, threadId).catch(() => {
            // Non-fatal: a stale session may already be dead. Proceed
            // with the resume regardless.
          });
          // Clear the old slice so the transcript doesn't flash the
          // previous chat's messages while the resumed session boots.
          useAgentChatStore.getState().resetThread(threadId);
        }
        // Hydrate → start with the row's cursor → seed the slice → honest
        // toast. Shared with the draft surface, which resumes without a
        // pane of its own to stop first.
        await launchResumedRecord(
          provider,
          { ...record, sdk_session_id: sdkSessionId },
          { paneId: target.paneId, cwd: target.cwd },
        );
      } catch (error) {
        toast.error(`Failed to reopen chat: ${error}`);
      }
    },
    [paneId, threadId, provider],
  );

  const handleSelect = useCallback(
    (record: AgentChatSessionRecord) =>
      // The row's OWN directory, not the pane's: `upsert_agent_chat_session`
      // writes back whatever cwd the start carries, so passing the pane's
      // would rewrite the thread's recorded folder. Rows the dropdown lists
      // already match this pane (it queries on an exact cwd match); a row
      // written before the column existed has none and heals to the pane's.
      resumeRecord(record, { paneId, cwd: record.cwd ?? cwd }),
    [cwd, paneId, resumeRecord],
  );

  // Run an adoption the backend has already completed: stop what the
  // pane was doing, hydrate the minted thread, launch it. Split out of
  // the click handler so a foreign-project confirmation can resume from
  // here without adopting the same conversation twice.
  const launchAdoptedSession = useCallback(
    async (
      session: AdoptableAgentSession,
      result: AdoptExternalSessionResult,
    ) => {
      // The pane the conversation RUNS in, which is not always the one
      // it was requested from: when the session lives in another folder
      // the backend has already opened a chat pane rooted there, bound
      // the thread to it and focused it. Starting here instead would
      // leave that tab blank and hide the thread from this pane's
      // history dropdown, which filters on the pane's own cwd.
      const onThisPane = result.pane_id === paneId;
      try {
        // The RUNNING pane's current permission mode wins — the external
        // session's own mode is deliberately never restored, so adopting
        // a conversation can't silently widen (or narrow) what the agent
        // is allowed to do. A re-homed pane is brand new and has no mode
        // of its own, so it launches in the provider default, exactly
        // like any other fresh chat tab. Read it before the reset below
        // drops the slice.
        const store = useAgentChatStore.getState();
        const currentMode =
          onThisPane && threadId
            ? store.threads[threadId]?.permissionMode
            : undefined;
        if (onThisPane && threadId) {
          await agentChatStopSession(provider, threadId).catch(() => {
            // Non-fatal: a stale session may already be dead.
          });
          store.resetThread(threadId);
        }

        // Name the directory whenever the conversation did not land on
        // the pane the user clicked from — a tab appearing elsewhere
        // needs to say where it went, and so does the rare case where
        // re-homing failed and the backend fell back to this pane.
        const where =
          !onThisPane || result.foreign_project ? ` in ${result.cwd}` : "";
        // The backend already minted the thread, bound it to the pane
        // rooted at the session's folder and wrote the "resumed outside
        // Codemux" divider; the shared launcher hydrates from THAT id,
        // starts with the session's cursor and toasts honestly. When the
        // folder belongs to another project the user has already agreed
        // to the move — the gate upstream of this function is what asks.
        await launchAdoptedThread(
          provider,
          result,
          { model: null, permissionMode: currentMode ?? null },
          where,
        );
      } catch (error) {
        toast.error(`Failed to resume "${session.title}": ${error}`);
      }
    },
    [paneId, provider, threadId],
  );

  // Adopt, then either switch (already ours), park for confirmation
  // (another project's directory), or launch.
  const runAdoption = useCallback(
    async (session: AdoptableAgentSession, allowForeignProject: boolean) => {
      let result;
      try {
        result = await agentChatAdoptExternalSession(
          paneId,
          provider,
          toExternalAgentSession(session),
        );
      } catch (error) {
        toast.error(`Failed to resume "${session.title}": ${error}`);
        return;
      }

      // Already in Codemux (R5): a second adoption would fork the same
      // conversation across two threads, so switch to the one that owns
      // it and reuse the ordinary resume path — that thread has local
      // history worth hydrating.
      if (result.existing_thread_id) {
        const record = await agentChatGetSession(
          result.existing_thread_id,
        ).catch(() => null);
        if (!record) {
          toast.error(
            `"${session.title}" is already in Codemux, but its thread could not be opened.`,
          );
          return;
        }
        // Switch, never re-point: the row keeps its own directory and
        // runs on the pane the backend says owns it (the pane that
        // thread already lives in). Resuming it on THIS pane's cwd
        // would rewrite the thread's recorded folder on the upsert and
        // put the agent in the wrong tree.
        await resumeRecord(record, {
          paneId: result.pane_id,
          cwd: record.cwd ?? result.cwd,
        });
        return;
      }

      // R4, defence in depth: the discovery row said this lived in the
      // pane's checkout but the backend disagrees. The thread exists
      // now, yet starting it would re-point the pane at another
      // project's directory — park it and let the user decide.
      if (result.foreign_project && !allowForeignProject) {
        setForeignPrompt({ session, cwd: result.cwd, adopted: result });
        return;
      }

      await launchAdoptedSession(session, result);
    },
    [paneId, provider, resumeRecord, launchAdoptedSession],
  );

  const handleAdoptExternalSession = useCallback(
    async (session: AdoptableAgentSession) => {
      // R4: a conversation from an unrelated project must never
      // re-point the pane on a single click. Gate BEFORE the adopt call
      // so nothing is minted until the user agrees. Rows Codemux
      // already owns switch to their own thread and never move the
      // pane, so they skip the gate.
      if (!session.existing_thread_id && !session.same_repo) {
        setForeignPrompt({ session, cwd: session.cwd, adopted: null });
        return;
      }
      await runAdoption(session, false);
    },
    [runAdoption],
  );

  const confirmForeignProjectAdopt = useCallback(async () => {
    const pending = foreignPrompt;
    if (!pending) return;
    setForeignPrompt(null);
    if (pending.adopted) {
      await launchAdoptedSession(pending.session, pending.adopted);
      return;
    }
    await runAdoption(pending.session, true);
  }, [foreignPrompt, launchAdoptedSession, runAdoption]);

  const dismissForeignProjectAdopt = useCallback(
    () => setForeignPrompt(null),
    [],
  );

  const handleNewChat = useCallback(async () => {
    if (!cwd) {
      toast.error("Cannot start a new chat: no working directory.");
      return;
    }
    try {
      if (threadId) {
        await agentChatStopSession(provider, threadId).catch(() => {});
        // Clear the old slice so the transcript doesn't flash the
        // previous chat's messages while the new session boots.
        useAgentChatStore.getState().resetThread(threadId);
      }
      const newLocalThreadId = `chat-${paneId}-${Date.now()}`;
      // Launch in the provider default mode (Full access) — the same mode
      // the fresh store slice advertises in the footer pill. Passing `null`
      // here would boot the provider in `default` (prompt-for-every-tool)
      // while the pill still reads "Full access", the drift this hook exists
      // to close.
      const startMode = defaultPermissionModeForProvider(provider);
      const startModel = defaultModelForProvider(provider);
      const newThreadId = await agentChatStartSession(paneId, provider, {
        thread_id: newLocalThreadId,
        cwd,
        model: null,
        resume_cursor: null,
        permission_mode: startMode,
        fast_mode: false,
        additional_directories: [],
        env: null,
      });
      // Seed the new slice the same way the pane's fresh-boot path does, so
      // the pickers render immediately and `permissionMode` /
      // `sessionLaunchMode` agree (a mismatch is read as a user mode change
      // and triggers a spurious silent restart).
      const store = useAgentChatStore.getState();
      store.ensureThread(newThreadId);
      store.setModel(newThreadId, startModel);
      if (startMode !== null) {
        store.setPermissionMode(newThreadId, startMode);
      }
      store.setFastMode(newThreadId, false);
      store.setSessionLaunchMode(newThreadId, startMode);
    } catch (error) {
      toast.error(`Failed to start new chat: ${error}`);
    }
  }, [cwd, paneId, threadId, provider]);

  return {
    cwd,
    provider,
    handleSelect,
    handleAdoptExternalSession,
    foreignProjectPrompt: foreignPrompt
      ? { session: foreignPrompt.session, cwd: foreignPrompt.cwd }
      : null,
    confirmForeignProjectAdopt,
    dismissForeignProjectAdopt,
    handleNewChat,
  };
}
