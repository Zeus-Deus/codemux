import { defaultModelForProvider } from "@/components/chat/pickers/ModelPicker";
import { toast } from "@/lib/toast";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import {
  agentChatListMessagesAfter,
  agentChatStartSession,
  type AdoptExternalSessionResult,
  type AgentChatSessionRecord,
} from "@/tauri/commands";
import type { AgentChatProviderKind } from "@/tauri/types";

import { defaultPermissionModeForProvider } from "./capability-defaults";
import { sessionDisplayTitle } from "./session-history";

/**
 * Pane-free halves of "run a persisted conversation on a pane".
 *
 * Two surfaces resume conversations: a live chat pane (`/resume` and
 * the history dropdown, via `useAgentChatSessionActions`) and the new
 * workspace draft, which has no pane yet and only learns which one to
 * use after the adoption resolves the session's directory. Both need
 * exactly the same tail — hydrate what the backend persisted, start the
 * provider with the resume cursor, seed the store slice so the footer
 * pickers agree with the launch, and tell the user honestly whether the
 * transcript above the composer is real — so it lives here once. What
 * differs (stopping the pane's current session, asking before jumping
 * projects, clearing a draft) stays with the caller.
 */

/** Launch config for an adopted conversation. The external session's
 *  own permission mode is deliberately NOT part of this: the caller
 *  passes whatever Codemux currently shows (the pane's mode, the
 *  draft's picker), so adopting can never silently widen or narrow
 *  what the agent may do. */
export interface AdoptedLaunchConfig {
  /** `null` launches the provider default, like a fresh chat tab. */
  model: string | null;
  /** `null` heals to the provider default (OpenCode stays null — it has
   *  no chat-side permission picker). */
  permissionMode: string | null;
  effort?: string | null;
  contextWindow?: string | null;
  fastMode?: boolean;
}

/** Where the resumed session runs. `cwd` MUST be the thread's own
 *  directory: `agent_chat_start_session` persists it back onto the row,
 *  so passing any other pane's would silently re-point the thread. */
export interface ResumeTarget {
  paneId: string;
  cwd: string;
}

/** Copy `sourceThreadId`'s persisted rows into the `localThreadId`
 *  slice so the pane renders history immediately instead of going blank
 *  while the provider boots. Never throws: a failed read only means the
 *  transcript is not shown — the provider still holds the conversation —
 *  and the caller's toast is what tells the user which it is. */
export async function hydrateThreadFromRows(
  sourceThreadId: string,
  localThreadId: string,
  provider: AgentChatProviderKind,
): Promise<boolean> {
  try {
    // Cursor read, single replay: the rows carry their durable ids, so
    // the slice starts with a resume cursor instead of cold-replaying
    // again on its first remount.
    const rows = await agentChatListMessagesAfter(sourceThreadId, null);
    if (rows.length === 0) return false;
    useAgentChatStore
      .getState()
      .hydrateThread(localThreadId, rows, { provider });
    return true;
  } catch (err) {
    console.warn("[agent-chat] hydrate on resume failed:", err);
    return false;
  }
}

function seedLaunchedSlice(
  threadId: string,
  launch: {
    model: string;
    permissionMode: string | null;
    effort?: string | null;
    contextWindow?: string | null;
    fastMode: boolean;
  },
): void {
  // `permissionMode` and `sessionLaunchMode` MUST agree — a mismatch is
  // read as "the user changed the mode" and triggers a spurious silent
  // restart.
  const store = useAgentChatStore.getState();
  store.ensureThread(threadId);
  store.setModel(threadId, launch.model);
  if (launch.effort !== undefined) store.setEffort(threadId, launch.effort);
  if (launch.contextWindow !== undefined) {
    store.setContextWindow(threadId, launch.contextWindow);
  }
  store.setFastMode(threadId, launch.fastMode);
  if (launch.permissionMode !== null) {
    store.setPermissionMode(threadId, launch.permissionMode);
  }
  store.setSessionLaunchMode(threadId, launch.permissionMode);
}

/**
 * Run a conversation the backend has just adopted from the provider's
 * CLI. The backend already minted `result.thread_id`, bound it to
 * `result.pane_id` (the pane rooted at `result.cwd`) and wrote the
 * "resumed from the terminal" divider into it, so this hydrates from
 * THAT id, starts the provider with the session's resume cursor and
 * seeds the slice from `launch`.
 *
 * `where` is appended to the toast (" in /some/dir") whenever the
 * caller wants the directory named — a conversation that opened
 * somewhere other than where the user clicked must say so.
 *
 * Throws when the provider fails to start; the caller owns that toast
 * because only it knows what the user was doing.
 */
export async function launchAdoptedThread(
  provider: AgentChatProviderKind,
  result: AdoptExternalSessionResult,
  launch: AdoptedLaunchConfig,
  where = "",
): Promise<string> {
  const dividerVisible = await hydrateThreadFromRows(
    result.thread_id,
    result.thread_id,
    provider,
  );
  // OpenCode has no chat-side permission picker.
  const resolvedMode =
    provider === "opencode"
      ? null
      : (launch.permissionMode ?? defaultPermissionModeForProvider(provider));
  const fastMode = launch.fastMode ?? false;
  // `cwd` and `pane_id` both come from the RESULT and are always a
  // matched pair: adoption attaches to the folder the conversation
  // already lives in (never creating a worktree) and the backend hands
  // back the pane rooted at that folder.
  const threadId = await agentChatStartSession(result.pane_id, provider, {
    thread_id: result.thread_id,
    cwd: result.cwd,
    model: launch.model,
    resume_cursor: { resume: result.sdk_session_id },
    permission_mode: resolvedMode,
    ...(launch.effort !== undefined ? { effort: launch.effort } : {}),
    ...(launch.contextWindow !== undefined
      ? { context_window: launch.contextWindow }
      : {}),
    fast_mode: fastMode,
    additional_directories: [],
    env: null,
  });
  seedLaunchedSlice(threadId, {
    model: launch.model ?? defaultModelForProvider(provider),
    permissionMode: resolvedMode,
    effort: launch.effort,
    contextWindow: launch.contextWindow,
    fastMode,
  });

  if (result.resume_divider_written && dividerVisible) {
    toast.success(
      `Resumed "${result.title}"${where} — the agent has the full history`,
    );
  } else {
    // Never a success toast over a blank transcript: the agent still
    // holds the conversation, but nothing above the composer says so,
    // and the user must be told which it is.
    toast.warning(
      `Resumed "${result.title}"${where}, but the earlier transcript isn't shown here — the agent still has it. This thread starts from your next message.`,
    );
  }
  return threadId;
}

/**
 * Reopen a persisted Codemux conversation on `target`: hydrate its
 * transcript into a fresh local thread, then start the provider with
 * the row's `sdk_session_id` as the resume cursor. The record's own
 * picker config (model / effort / context / permission mode) rides
 * through so the session runs the way the user last left it; a NULL
 * permission mode heals to the provider default, matching what the
 * footer pill shows for NULL rows.
 *
 * The caller must have checked `record.sdk_session_id` — a row that
 * never finished its first turn has nothing to resume. Throws when the
 * provider fails to start.
 */
export async function launchResumedRecord(
  provider: AgentChatProviderKind,
  record: AgentChatSessionRecord & { sdk_session_id: string },
  target: ResumeTarget,
): Promise<string> {
  const newLocalThreadId = `chat-${target.paneId}-${Date.now()}`;
  const transcriptVisible = await hydrateThreadFromRows(
    record.thread_id,
    newLocalThreadId,
    provider,
  );
  const resolvedModel = record.model ?? defaultModelForProvider(provider);
  // OpenCode has no chat-side permission picker: launch with null even
  // if the record carries a stale cross-provider token.
  const resolvedMode =
    provider === "opencode"
      ? null
      : (record.permission_mode ?? defaultPermissionModeForProvider(provider));
  const threadId = await agentChatStartSession(target.paneId, provider, {
    thread_id: newLocalThreadId,
    cwd: target.cwd,
    model: record.model,
    resume_cursor: { resume: record.sdk_session_id },
    permission_mode: resolvedMode,
    effort: record.effort,
    context_window: record.context_window,
    fast_mode: record.fast_mode ?? false,
    additional_directories: [],
    env: null,
  });
  seedLaunchedSlice(threadId, {
    model: resolvedModel,
    permissionMode: resolvedMode,
    effort: record.effort,
    contextWindow: record.context_window,
    fastMode: record.fast_mode ?? false,
  });

  const title = sessionDisplayTitle(record);
  if (transcriptVisible) {
    toast.success(`Resumed "${title}" — agent has the full history`);
  } else {
    toast.warning(
      `Resumed "${title}", but the earlier transcript isn't shown here — the agent still has it. This thread starts from your next message.`,
    );
  }
  return threadId;
}
