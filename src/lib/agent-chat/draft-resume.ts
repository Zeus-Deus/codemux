import { toast } from "@/lib/toast";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useAppStore } from "@/stores/app-store";
import { useChatDraftStore, type DraftId } from "@/stores/chat-draft-store";
import {
  activatePane,
  activateWorkspace,
  agentChatAdoptExternalSession,
  agentChatCreatePane,
  agentChatGetSession,
  createEmptyWorkspaceResult,
  importWorktreeWorkspace,
  listWorktrees,
  type AdoptableAgentSession,
  type AgentChatSessionRecord,
} from "@/tauri/commands";
import type {
  AgentChatProviderKind,
  AppStateSnapshot,
  PaneNodeSnapshot,
  WorkspaceSnapshot,
  WorktreeInfo,
} from "@/tauri/types";

import {
  launchAdoptedThread,
  launchResumedRecord,
} from "./adopt-external-session";
import { CLEAR_AFTER_PROMOTION_MS } from "./draft-preset-launch";
import { toExternalAgentSession } from "./external-sessions";
import { effectivePermissionMode } from "./materialize";
import { formatProviderError } from "./provider-error";
import { sessionDisplayTitle } from "./session-history";

/**
 * Continue a conversation the agent's CLI started in a terminal, from
 * the new-workspace draft — with no first prompt and no guessing which
 * folder it ran in.
 *
 * The session already knows its directory, so the draft's own location
 * picker is irrelevant here: the workspace is resolved from the
 * session's `cwd` (reused when one is already open there, otherwise
 * created AT that folder), a chat pane is opened in it, the session is
 * adopted into a thread bound to that pane, and the provider starts
 * with the session's resume cursor. No turn is sent; the transcript
 * opens on the "resumed from the terminal" divider and the user takes
 * it from there.
 *
 * Rule R1 — never create a git worktree. A session that lives in a
 * linked worktree is attached where it is; a workspace is anchored at
 * that existing checkout, never forked from it.
 */

export type DraftResumeResult =
  | {
      success: true;
      workspaceId: string;
      paneId: string;
      threadId: string;
    }
  | { success: false; error: string };

/** Strip trailing separators so `/a/b/` and `/a/b` compare equal. */
function normalizeDirectory(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  return trimmed.length > 0 ? trimmed : path;
}

/**
 * The open, local workspace already anchored at `cwd`, if any. Matches
 * on the workspace's own directory or its worktree path. Remote and
 * attach-in-place workspaces are skipped: a path string on another host
 * is not this folder, and an attached one is not ours to open panes in.
 */
export function findWorkspaceAtDirectory(
  appState: AppStateSnapshot | null,
  cwd: string,
): WorkspaceSnapshot | null {
  if (!appState) return null;
  const wanted = normalizeDirectory(cwd);
  return (
    appState.workspaces.find((ws) => {
      if (ws.host_id !== null && ws.host_id !== undefined) return false;
      if (ws.attach_only) return false;
      const candidates = [ws.cwd, ws.worktree_path].filter(
        (value): value is string => typeof value === "string" && value.length > 0,
      );
      return candidates.some((value) => normalizeDirectory(value) === wanted);
    }) ?? null
  );
}

function findPaneInTree(
  root: PaneNodeSnapshot | null,
  threadId: string,
): string | null {
  if (!root) return null;
  if (root.kind === "agent_chat") {
    return root.thread_id === threadId ? root.pane_id : null;
  }
  if (root.kind === "split") {
    for (const child of root.children) {
      const found = findPaneInTree(child, threadId);
      if (found) return found;
    }
  }
  return null;
}

/** The chat pane already bound to `threadId`, wherever it lives. */
export function findPaneForThread(
  appState: AppStateSnapshot | null,
  threadId: string,
): { workspaceId: string; paneId: string } | null {
  if (!appState) return null;
  for (const ws of appState.workspaces) {
    for (const surface of ws.surfaces ?? []) {
      const paneId = findPaneInTree(surface.root, threadId);
      if (paneId) return { workspaceId: ws.workspace_id, paneId };
    }
  }
  return null;
}

/**
 * The branch checked out in `cwd` when it is a LINKED git worktree —
 * null for the main checkout, a folder that is not a git worktree, a
 * detached-HEAD worktree (nothing to title the workspace after), or
 * when the listing fails.
 *
 * `git worktree list` answers the same from any checkout of the repo
 * and reports the main checkout first, so the session's own folder is
 * enough to ask — no root lookup, no shelling out from here. The match
 * is on the path git reports; a session that ran in a SUBFOLDER of a
 * worktree, or reached it through a different spelling (symlink), does
 * not match and takes the plain path, which fails safe.
 */
async function linkedWorktreeBranch(cwd: string): Promise<string | null> {
  let worktrees: WorktreeInfo[];
  try {
    worktrees = await listWorktrees(cwd);
  } catch (err) {
    console.warn(
      "[draft-resume] listWorktrees failed; opening a plain workspace:",
      errorMessage(err),
    );
    return null;
  }
  const wanted = normalizeDirectory(cwd);
  const linked = worktrees
    .slice(1)
    .find((wt) => normalizeDirectory(wt.path) === wanted);
  return linked?.branch || null;
}

/**
 * The workspace a session at `cwd` should run in: the one already open
 * there, else a new one anchored at exactly that folder.
 *
 * A linked worktree is IMPORTED (`import_worktree_workspace` with the
 * "empty" layout): the workspace carries the worktree metadata, so the
 * sidebar shows it as the worktree it is and closing it offers to remove
 * the checkout, and — like `create_empty_workspace` — it opens with no
 * terminal beside the chat pane attached afterward. The existing
 * on-disk worktree is adopted in place; nothing is ever created with
 * `git worktree add` (R1).
 *
 * Everything else — the main checkout, a folder that is not a git
 * worktree, or a worktree the listing could not identify (see
 * `linkedWorktreeBranch`) — goes through `create_empty_workspace`,
 * which stores the folder verbatim as the workspace cwd and resolves
 * `project_root` through the folder's git metadata.
 */
export async function resolveWorkspaceForDirectory(
  cwd: string,
): Promise<{ workspaceId: string; cwd: string; reused: boolean }> {
  const existing = findWorkspaceAtDirectory(
    useAppStore.getState().appState,
    cwd,
  );
  if (existing) {
    return { workspaceId: existing.workspace_id, cwd, reused: true };
  }
  const branch = await linkedWorktreeBranch(cwd);
  if (branch) {
    const workspaceId = await importWorktreeWorkspace(cwd, branch, "empty");
    return { workspaceId, cwd, reused: false };
  }
  const created = await createEmptyWorkspaceResult(cwd);
  if (!created.workspaceId) {
    throw new Error(`Could not open a workspace at ${cwd}`);
  }
  return { workspaceId: created.workspaceId, cwd: created.cwd ?? cwd, reused: false };
}

function errorMessage(err: unknown): string {
  if (err instanceof Error) return formatProviderError(err.message);
  if (typeof err === "string") return formatProviderError(err);
  try {
    return formatProviderError(JSON.stringify(err));
  } catch {
    return "Unknown error";
  }
}

/** The same transition cleanup a composer submit runs once the draft
 *  has become a real thread: record the promotion, drop the chips the
 *  draft was holding (the adopted thread has its own id, so nothing
 *  inherits them), swap the router to the live workspace, and sweep the
 *  draft entry after the grace period. */
function finishDraft(
  draftId: DraftId,
  draftThreadId: string,
  promotedTo: { workspaceId: string; paneId: string; threadId: string },
): void {
  const state = useChatDraftStore.getState();
  state.markPromoted(draftId, promotedTo);
  useAgentChatStore.getState().clearStagedAttachments(draftThreadId);
  state.setActiveDraft(null);
  setTimeout(
    () => useChatDraftStore.getState().clearDraft(draftId),
    CLEAR_AFTER_PROMOTION_MS,
  );
}

async function activateWorkspaceQuietly(workspaceId: string): Promise<void> {
  try {
    await activateWorkspace(workspaceId);
  } catch (err) {
    console.warn(
      "[draft-resume] activateWorkspace failed (non-fatal):",
      errorMessage(err),
    );
  }
}

/**
 * A session Codemux already owns a thread for: switch to it instead of
 * adopting a second copy. When a pane is still bound to the thread,
 * focusing it is the whole job — the pane brings its own session back.
 * Otherwise the thread gets a fresh pane in its own directory and the
 * ordinary record resume (hydrate, then start from its cursor).
 */
async function switchToOwnedThread(
  provider: AgentChatProviderKind,
  session: AdoptableAgentSession,
  record: AgentChatSessionRecord,
): Promise<{ workspaceId: string; paneId: string; threadId: string }> {
  const appState = useAppStore.getState().appState;
  const bound = findPaneForThread(appState, record.thread_id);
  if (bound) {
    await activateWorkspaceQuietly(bound.workspaceId);
    await activatePane(bound.paneId).catch(() => {
      // Non-fatal: the workspace is active; the pane may already be.
    });
    toast.success(`Switched to "${sessionDisplayTitle(record)}"`);
    return { ...bound, threadId: record.thread_id };
  }

  const sdkSessionId = record.sdk_session_id;
  if (!sdkSessionId) {
    throw new Error(
      `"${session.title}" is already in Codemux but hasn't finished its first turn yet — can't resume.`,
    );
  }
  // The row's own directory, never anything the draft picked: the start
  // persists its cwd back onto the row.
  const cwd = record.cwd ?? session.cwd;
  const recordWorkspaceStillOpen = appState?.workspaces.some(
    (ws) => ws.workspace_id === record.workspace_id,
  );
  const workspaceId = recordWorkspaceStillOpen
    ? record.workspace_id
    : (await resolveWorkspaceForDirectory(cwd)).workspaceId;
  const paneId = await agentChatCreatePane(workspaceId, provider, cwd);
  const threadId = await launchResumedRecord(
    provider,
    { ...record, sdk_session_id: sdkSessionId },
    { paneId, cwd },
  );
  await activateWorkspaceQuietly(workspaceId);
  return { workspaceId, paneId, threadId };
}

/**
 * Resume `session` from the draft identified by `draftId`. Reads the
 * draft fresh from the store, so the permission mode and model it
 * launches with are whatever the composer's footer shows right now —
 * never the terminal session's own mode. Reports failure instead of
 * throwing; workspace and pane resources created before a failure are
 * left in place, matching the composer submit path.
 */
export async function resumeExternalSessionFromDraft(
  draftId: DraftId,
  session: AdoptableAgentSession,
): Promise<DraftResumeResult> {
  const state = useChatDraftStore.getState();
  const draft = state.draftsById[draftId];
  if (!draft) return { success: false, error: "Draft no longer exists" };
  if (draft.promoting) {
    return { success: false, error: "The draft is already being started" };
  }
  const provider = draft.provider;
  state.markPromoting(draftId);

  try {
    // Already in Codemux (R5): switch, never adopt twice.
    if (session.existing_thread_id) {
      const record = await agentChatGetSession(session.existing_thread_id).catch(
        () => null,
      );
      if (!record) {
        throw new Error(
          `"${session.title}" is already in Codemux, but its thread could not be opened.`,
        );
      }
      const switched = await switchToOwnedThread(provider, session, record);
      finishDraft(draftId, draft.threadId, switched);
      return { success: true, ...switched };
    }

    // 1. The workspace at the session's OWN folder — reused, imported
    //    (an existing linked worktree), or created there. Never a NEW
    //    worktree (R1).
    const workspace = await resolveWorkspaceForDirectory(session.cwd);
    // Pin the draft to it immediately: from here on nothing may
    // re-point the draft at the sidebar's active workspace or flip it
    // to a worktree checkout while the adoption is in flight.
    useChatDraftStore.getState().lockDraftTarget(draftId, {
      kind: "existing_workspace",
      workspaceId: workspace.workspaceId,
    });

    // 2. A chat pane rooted at that folder, so the backend attaches the
    //    conversation to this pane instead of opening another one.
    const paneId = await agentChatCreatePane(
      workspace.workspaceId,
      provider,
      workspace.cwd,
    );

    // 3. Adopt: mints the thread bound to the pane and writes the
    //    "resumed from the terminal" divider. Starts nothing.
    const result = await agentChatAdoptExternalSession(
      paneId,
      provider,
      toExternalAgentSession(session),
    );

    let threadId: string;
    if (result.existing_thread_id) {
      // Raced: another surface adopted this conversation between the
      // discovery listing and now. Same answer as above — switch to the
      // thread that owns it rather than forking it.
      const record = await agentChatGetSession(result.existing_thread_id).catch(
        () => null,
      );
      if (!record) {
        throw new Error(
          `"${session.title}" is already in Codemux, but its thread could not be opened.`,
        );
      }
      const sdkSessionId = record.sdk_session_id;
      if (!sdkSessionId) {
        throw new Error(
          `"${session.title}" is already in Codemux but hasn't finished its first turn yet — can't resume.`,
        );
      }
      threadId = await launchResumedRecord(
        provider,
        { ...record, sdk_session_id: sdkSessionId },
        { paneId: result.pane_id, cwd: record.cwd ?? result.cwd },
      );
    } else {
      // 4. Start with the session's cursor, in the mode the draft's
      //    footer shows. The draft had no committed location, so the
      //    toast always names the folder the conversation opened in —
      //    that is the one thing the user could not see before picking.
      threadId = await launchAdoptedThread(
        provider,
        result,
        {
          model: draft.model,
          permissionMode: effectivePermissionMode(draft),
          effort: draft.effort,
          contextWindow: draft.contextWindow,
          fastMode: draft.fastMode ?? false,
        },
        ` in ${result.cwd}`,
      );
    }

    // 5. Show it. Failure here is non-fatal — the workspace exists and
    //    reconciles on the next app-state emit.
    await activateWorkspaceQuietly(workspace.workspaceId);

    const promotedTo = {
      workspaceId: workspace.workspaceId,
      paneId: result.pane_id,
      threadId,
    };
    finishDraft(draftId, draft.threadId, promotedTo);
    return { success: true, ...promotedTo };
  } catch (err) {
    const message = errorMessage(err);
    // Re-enable the composer without pretending a message failed to
    // send; the caller's toast names the session.
    useChatDraftStore.getState().abortPromotion(draftId);
    return { success: false, error: message };
  }
}
