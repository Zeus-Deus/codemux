import {
  activateWorkspace,
  agentChatCreatePane,
  agentChatSendTurn,
  agentChatStartSession,
  applyPreset,
  createEmptyWorkspace,
  getHomeDir,
  renameWorkspace,
} from "@/tauri/commands";
import {
  DEFAULT_THREAD_PERMISSION_MODE,
  type ChatMode,
} from "@/stores/agent-chat-store";
import { useAppStore } from "@/stores/app-store";
import type { ChatDraft, DraftId } from "@/stores/chat-draft-store";
import type { TerminalPreset } from "@/tauri/types";

import { deriveTitleFromFirstMessage } from "./derive-title";
import { applyAllPrefixes } from "./mode-prefix";

/**
 * Mutations performed as `materializeAndSend` progresses. Kept as a bag
 * of plain functions so the flow is trivially mockable in tests and so
 * the lib module stays free of direct store imports.
 */
export interface MaterializeActions {
  /** Marks the draft as in-flight. Called first. */
  markPromoting: (id: DraftId) => void;
  /** Records the materialised workspace / pane / thread on the draft. */
  markPromoted: (
    id: DraftId,
    promotedTo: NonNullable<ChatDraft["promotedTo"]>,
  ) => void;
  /** Records a send failure so the composer can surface retry UI. */
  markSendFailed: (id: DraftId, error: string) => void;
  /** Idempotent — ensures a slice exists on the agent-chat store for
   *  the pre-minted `thread_id` so the optimistic user message has a
   *  home before the live pane mounts. */
  ensureThread: (threadId: string) => void;
  /** Writes the optimistic user message into the agent-chat slice so
   *  the live pane renders it as soon as it takes over. */
  appendUserMessage: (threadId: string, text: string) => void;
  // ── Stage C Effort-lock fix: mirror the draft's session config
  //    onto the agent-chat slice so the ReasoningPicker (which gates
  //    on `slice.model`) renders correctly when `AgentChatPane`
  //    mounts post-materialize, and `restartSessionWith` has a
  //    non-null launch mode to compare against.
  /** Seed the slice's `model` with the draft's model id. */
  setModel: (threadId: string, model: string | null) => void;
  /** Seed the slice's user-visible `permissionMode`. */
  setPermissionMode: (threadId: string, mode: string) => void;
  /** Seed the slice's `sessionLaunchMode` — the permission mode the
   *  session was actually started under. Used by the restart
   *  detection (`sessionLaunchMode !== permissionMode` → restart). */
  setSessionLaunchMode: (threadId: string, mode: string) => void;
  /** Seed the slice's reasoning/effort level. `null` clears to default. */
  setEffort: (threadId: string, effort: string | null) => void;
  /** Seed the slice's context-window selection. `null` clears. */
  setContextWindow: (threadId: string, contextWindow: string | null) => void;
  /** Seed the slice's composer mode pill. Stage 3 onward. */
  setMode: (threadId: string, mode: ChatMode) => void;
}

export type MaterializeResult =
  | {
      success: true;
      workspaceId: string;
      paneId: string;
      threadId: string;
    }
  | { success: false; error: string };

/**
 * Promote a draft into a real workspace and send the first turn.
 *
 * Order of operations:
 *   1. Mark the draft as promoting (clears any prior `lastSendError`).
 *   2. Resolve-or-create the target workspace.
 *   3. Create a chat pane on that workspace.
 *   4. Seed the agent-chat slice with an optimistic user message so
 *      the live pane renders it without waiting for a backend echo.
 *   5. Start the provider session using the draft's pre-minted
 *      `thread_id` so the live pane mounts against the same slice and
 *      transition is flicker-free.
 *   6. Send the first turn.
 *   7. Activate the workspace so the sidebar + main area flip from the
 *      draft surface to the real pane tree.
 *
 * Failure policy: any step returns an error, which the caller surfaces
 * via `markSendFailed` and a retry affordance. Workspace + pane
 * resources created by earlier steps are intentionally NOT rolled back
 * — the user can retry the send, close the workspace, or dismiss.
 */
export async function materializeAndSend(
  draft: ChatDraft,
  text: string,
  cwd: string,
  actions: MaterializeActions,
  /** Concatenated skill bodies extracted from `text` by the caller —
   *  caller parses against its own skills registry so this lib stays
   *  free of store imports. `null` when the draft mentions no skills. */
  skillBodies: string | null = null,
  /** Step 8 Stage 2 — pre-built attachment block from the draft's
   *  staged attachments. Caller is responsible for snapshotting
   *  `stagedAttachments` and running `buildAttachmentBlock` so this
   *  lib stays free of store imports. `null` when the draft has no
   *  attachments. */
  attachmentBlock: string | null = null,
  /** Step 8 Stage 6 — image attachments converted to the wire shape
   *  agent_chat_send_turn expects. Caller pre-extracts via
   *  `buildImagePayloads` so this lib stays free of store imports.
   *  Defaults to empty so existing call sites keep compiling. */
  images: Array<{ data: number[]; media_type: string }> = [],
): Promise<MaterializeResult> {
  actions.markPromoting(draft.draftId);

  // 1. Resolve the target workspace.
  let workspaceId: string;
  try {
    switch (draft.target.kind) {
      case "home": {
        const created = await createHomeRootedWorkspace(text);
        workspaceId = created;
        break;
      }
      case "project":
        workspaceId = await createEmptyWorkspace(draft.target.projectPath);
        break;
      case "existing_workspace":
        workspaceId = draft.target.workspaceId;
        break;
    }
  } catch (err) {
    const message = errorMessage(err);
    actions.markSendFailed(draft.draftId, message);
    return { success: false, error: message };
  }

  // 2. Create a chat pane on that workspace.
  let paneId: string;
  try {
    paneId = await agentChatCreatePane(workspaceId, draft.provider, cwd);
  } catch (err) {
    const message = errorMessage(err);
    actions.markSendFailed(draft.draftId, message);
    return { success: false, error: message };
  }

  // 3. Seed the transcript optimistically. `ensureThread` + slice
  //    creation happens implicitly inside `appendUserMessage` on the
  //    Zustand store (via `updateSlice` which defaults to `emptySlice`)
  //    but we call ensureThread explicitly so the slice is materialised
  //    even if the append is a no-op for some reason. We also mirror
  //    the draft's session config onto the slice so the composer's
  //    pickers (ReasoningPicker, PermissionModePicker) render their
  //    model-gated options when AgentChatPane mounts — otherwise the
  //    slice's `model` stays null and the pickers silently vanish.
  actions.ensureThread(draft.threadId);
  seedSliceFromDraft(draft, actions);
  actions.appendUserMessage(draft.threadId, text);

  // 4. Start the provider session with the draft's pre-minted thread
  //    id. The backend accepts the caller's id verbatim (verified on
  //    the Claude adapter at claude/mod.rs:162), so the live pane's
  //    later `ensureThread(pane.thread_id)` lands on the same slice we
  //    just seeded — no migration, no flicker.
  try {
    await agentChatStartSession(paneId, draft.provider, {
      thread_id: draft.threadId,
      cwd,
      model: draft.model,
      resume_cursor: null,
      permission_mode: effectivePermissionMode(draft),
      effort: draft.effort,
      context_window: draft.contextWindow,
      additional_directories: [],
      env: null,
    });
  } catch (err) {
    const message = errorMessage(err);
    actions.markSendFailed(draft.draftId, message);
    return { success: false, error: message };
  }

  // 5. Send the first turn. Mode wrappers (ASK / DEBUG) live SDK-side
  //    only — the transcript above stored the user's raw text so the
  //    framing we layered on top doesn't echo back into the UI. Skill
  //    bodies (if the user mentioned `/skill-name` tokens) were
  //    parsed by the caller and arrive via `skillBodies`.
  try {
    await agentChatSendTurn(draft.provider, {
      thread_id: draft.threadId,
      text: applyAllPrefixes(
        text,
        draft.mode,
        draft.effort,
        skillBodies,
        attachmentBlock,
      ),
      images,
      model_override: null,
      effort_override: draft.effort,
      permission_mode_override: null,
    });
  } catch (err) {
    const message = errorMessage(err);
    actions.markSendFailed(draft.draftId, message);
    return { success: false, error: message };
  }

  // 6. Activate the workspace so the sidebar and WorkspaceMain route
  //    to the live pane. Failure here is non-fatal: the workspace
  //    exists, its state will reconcile on the next app-state emit.
  try {
    await activateWorkspace(workspaceId);
  } catch (err) {
    console.warn(
      "[materialize] activateWorkspace failed (non-fatal):",
      errorMessage(err),
    );
  }

  actions.markPromoted(draft.draftId, {
    workspaceId,
    paneId,
    threadId: draft.threadId,
  });

  return { success: true, workspaceId, paneId, threadId: draft.threadId };
}

/**
 * Promote a draft into a real workspace and launch the chosen preset.
 *
 * This is the commit-on-preset-click path: when the user clicks a
 * preset tile while a draft is active, the draft's composer text
 * carries over as the preset's initial prompt, the draft's target
 * (home / project / existing) turns into a real workspace, and the
 * preset takes over that workspace's active surface.
 *
 * Dispatch depends on `preset.kind`:
 *   - `cli`      — delegate to the Rust `apply_preset` command,
 *                  which spawns a terminal pane and runs the preset's
 *                  shell commands with `initialPrompt` spliced in.
 *   - `chat_agent` — create an `agent_chat` pane, start a session with
 *                  the draft's model/permission/etc., and send the
 *                  first turn. Same sequence `materializeAndSend` uses,
 *                  but triggered by the preset button rather than the
 *                  composer's Enter key.
 *
 * Failure policy matches `materializeAndSend`: workspace / pane
 * resources created by earlier steps are NOT rolled back. The user
 * can retry via the composer (draft persists with its text + preset
 * selection until `markPromoted` fires).
 */
export async function materializeWithPreset(
  draft: ChatDraft,
  preset: TerminalPreset,
  initialPrompt: string,
  actions: MaterializeActions,
  /** Concatenated skill bodies extracted from `initialPrompt` by the
   *  caller. `null` when the prompt mentions no skills. */
  skillBodies: string | null = null,
): Promise<MaterializeResult> {
  actions.markPromoting(draft.draftId);

  // 1. Resolve the target workspace. Same branching as
  //    `materializeAndSend`: home → fresh workspace at $HOME with a
  //    message-derived title; project → create empty; existing → use
  //    as-is.
  let workspaceId: string;
  try {
    switch (draft.target.kind) {
      case "home": {
        const created = await createHomeRootedWorkspace(initialPrompt);
        workspaceId = created;
        break;
      }
      case "project":
        workspaceId = await createEmptyWorkspace(draft.target.projectPath);
        break;
      case "existing_workspace":
        workspaceId = draft.target.workspaceId;
        break;
    }
  } catch (err) {
    const message = errorMessage(err);
    actions.markSendFailed(draft.draftId, message);
    return { success: false, error: message };
  }

  // 2. Activate the workspace up front so the preset's new pane /
  //    terminal lands on a visible surface. Non-fatal if this
  //    rejects; state will reconcile on the next app-state emit.
  try {
    await activateWorkspace(workspaceId);
  } catch (err) {
    console.warn(
      "[materialize] activateWorkspace failed (non-fatal):",
      errorMessage(err),
    );
  }

  const prompt = initialPrompt.trim();

  if (preset.kind === "chat_agent") {
    // ── ChatAgent dispatch ──
    // Mirrors the hot path of `materializeAndSend`, but keyed off the
    // preset rather than composer Enter. The Rust `apply_preset`
    // command explicitly rejects `ChatAgent` presets; all the work
    // happens here in the frontend.
    let cwd: string;
    try {
      cwd = await resolveCwdForTarget(draft);
    } catch (err) {
      const message = errorMessage(err);
      actions.markSendFailed(draft.draftId, message);
      return { success: false, error: message };
    }

    let paneId: string;
    try {
      paneId = await agentChatCreatePane(workspaceId, draft.provider, cwd);
    } catch (err) {
      const message = errorMessage(err);
      actions.markSendFailed(draft.draftId, message);
      return { success: false, error: message };
    }

    // Seed optimistic transcript only when there IS a first turn. A
    // preset click with an empty composer just opens an empty chat
    // pane — no message to echo. Either way we seed the slice's
    // session config from the draft so the picker gates open.
    actions.ensureThread(draft.threadId);
    seedSliceFromDraft(draft, actions);
    if (prompt.length > 0) {
      actions.appendUserMessage(draft.threadId, prompt);
    }

    try {
      await agentChatStartSession(paneId, draft.provider, {
        thread_id: draft.threadId,
        cwd,
        model: draft.model,
        resume_cursor: null,
        permission_mode: effectivePermissionMode(draft),
        effort: draft.effort,
        context_window: draft.contextWindow,
        additional_directories: [],
        env: null,
      });
    } catch (err) {
      const message = errorMessage(err);
      actions.markSendFailed(draft.draftId, message);
      return { success: false, error: message };
    }

    if (prompt.length > 0) {
      try {
        await agentChatSendTurn(draft.provider, {
          thread_id: draft.threadId,
          text: applyAllPrefixes(prompt, draft.mode, draft.effort, skillBodies),
          model_override: null,
          effort_override: draft.effort,
          permission_mode_override: null,
        });
      } catch (err) {
        const message = errorMessage(err);
        actions.markSendFailed(draft.draftId, message);
        return { success: false, error: message };
      }
    }

    actions.markPromoted(draft.draftId, {
      workspaceId,
      paneId,
      threadId: draft.threadId,
    });
    return {
      success: true,
      workspaceId,
      paneId,
      threadId: draft.threadId,
    };
  }

  // ── CLI dispatch ──
  // Delegate to the existing Rust `apply_preset`, which spawns a
  // terminal pane and runs the preset's commands. For agent presets
  // like Claude Code, the CLI reads `initialPrompt` and launches its
  // own session inside the PTY.
  try {
    await applyPreset(
      workspaceId,
      preset.id,
      "current_terminal",
      prompt.length > 0 ? prompt : null,
    );
  } catch (err) {
    const message = errorMessage(err);
    actions.markSendFailed(draft.draftId, message);
    return { success: false, error: message };
  }

  // CLI presets don't give us a usable paneId / threadId from the
  // frontend — `apply_preset` spawns the pane server-side. The
  // promotedTo values are placeholders; the grace-period cleanup
  // still runs via DraftChatSurface's `setActiveDraft(null)` +
  // scheduled `clearDraft`.
  actions.markPromoted(draft.draftId, {
    workspaceId,
    paneId: "cli-preset",
    threadId: draft.threadId,
  });
  return {
    success: true,
    workspaceId,
    paneId: "cli-preset",
    threadId: draft.threadId,
  };
}

/** Mirror the draft's session config onto the agent-chat slice for
 *  the draft's pre-minted thread id. Called by both materialize
 *  entry points before appending the optimistic user message so the
 *  pickers that gate on `slice.model` (Effort, ContextWindow) render
 *  when `AgentChatPane` mounts, and `sessionLaunchMode` is set to a
 *  non-null value the restart-detection can compare against.
 *
 *  All setters are idempotent slice mutations — calling them against
 *  a slice that already has these values is a no-op.
 *
 *  Plan-mode coupling: when `draft.mode === "plan"` the effective
 *  session permission_mode is forced to `"plan"` so the SDK boots
 *  with write operations locked off; the user's picker value is
 *  stashed on the slice (as `modePriorPermissionMode` via
 *  AgentChatPane's activation handler) so "remove pill" can restore
 *  it later. */
function seedSliceFromDraft(
  draft: ChatDraft,
  actions: MaterializeActions,
): void {
  const effectiveMode = effectivePermissionMode(draft);
  actions.setModel(draft.threadId, draft.model);
  actions.setPermissionMode(draft.threadId, effectiveMode);
  actions.setSessionLaunchMode(draft.threadId, effectiveMode);
  if (draft.effort) actions.setEffort(draft.threadId, draft.effort);
  if (draft.contextWindow) {
    actions.setContextWindow(draft.threadId, draft.contextWindow);
  }
  actions.setMode(draft.threadId, draft.mode);
}

/** Resolve the permission_mode the SDK session should launch with.
 *  Plan and Ask both force `"plan"` so the SDK boots with write
 *  operations locked off (Plan because that's the contract; Ask
 *  because we want SDK-level read-only enforcement on top of the
 *  per-turn prompt wrapper that nudges the model away from
 *  ExitPlanMode). The picker is hidden behind the pill while either
 *  is active. Debug remains a state-only flip until Stage 6. */
export function effectivePermissionMode(draft: ChatDraft): string {
  if (draft.mode === "plan" || draft.mode === "ask") return "plan";
  return draft.permissionMode ?? DEFAULT_THREAD_PERMISSION_MODE;
}

/** Create a fresh workspace rooted at the cached `$HOME`, then rename
 *  it to a title derived from the first message.
 *
 *  Throws if the `$HOME` cache isn't hydrated yet — caller converts
 *  that into a `markSendFailed` via the outer try/catch. Hydration
 *  happens at App mount, so in practice the cache is always ready by
 *  the time a user presses Enter.
 *
 *  The rename is best-effort: a failure here leaves the workspace
 *  with its default path-basename title (`"zeus"` on my box) rather
 *  than aborting the whole send. Matches the locked "no rollback on
 *  post-create failure" policy. */
async function createHomeRootedWorkspace(firstMessage: string): Promise<string> {
  const homeDir = useAppStore.getState().homeDir;
  if (!homeDir) {
    throw new Error("Home directory not loaded yet");
  }
  const workspaceId = await createEmptyWorkspace(homeDir, { skipSetup: true });
  const title = deriveTitleFromFirstMessage(firstMessage);
  if (title) {
    await renameWorkspace(workspaceId, title).catch((err) => {
      console.warn(
        "[materialize] title rename failed (non-fatal):",
        errorMessage(err),
      );
    });
  }
  return workspaceId;
}

/** Resolve the cwd a fresh session / terminal should launch under.
 *  Extracted so both the chat-agent and (future) CLI paths can share
 *  it without duplicating the target-switch. */
async function resolveCwdForTarget(draft: ChatDraft): Promise<string> {
  switch (draft.target.kind) {
    case "home":
      return await getHomeDir();
    case "project":
      return draft.target.projectPath;
    case "existing_workspace":
      // No cheap way to resolve a workspace's cwd from here without
      // pulling the app-store in. The frontend callers that construct
      // an existing_workspace target also know the cwd, so for now
      // the only path that produces one comes from DraftChatSurface,
      // which resolves it before calling materializeAndSend — if a
      // future picker wires existing_workspace through the preset
      // path, extend this branch to take cwd as a parameter.
      throw new Error(
        "existing_workspace target is not supported by the preset path yet",
      );
  }
}

function errorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  try {
    return JSON.stringify(err);
  } catch {
    return "Unknown error";
  }
}
