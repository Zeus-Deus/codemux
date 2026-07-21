import { materializeWithPreset } from "./materialize";
import type { MaterializeResult } from "./materialize";
import { resolveSkillBodies } from "./skill-tokens";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useChatDraftStore, type DraftId } from "@/stores/chat-draft-store";
import { selectActiveSkills, useSkillsStore } from "@/stores/skills-store";
import type { TerminalPreset } from "@/tauri/types";

/** Grace period between promoting a draft and sweeping its entry, so
 *  any in-flight UI can still observe the promotion before cleanup.
 *  Matches `CLEAR_AFTER_PROMOTION_MS` in DraftChatSurface. */
export const CLEAR_AFTER_PROMOTION_MS = 5000;

/**
 * Materialise an active draft by clicking a preset: commit the draft
 * (spawning only that preset's pane) via `materializeWithPreset`, then
 * run the same transition cleanup DraftChatSurface uses on composer
 * submit — clear the active draft immediately so the router swaps to
 * the live workspace, and sweep the draft entry after the grace period.
 *
 * Shared by the legacy `PresetBar` (draft mode) and the GUI-chrome
 * `DraftAgentLauncher` so both entry points stay behaviourally
 * identical. Reads fresh store state at call time so same-tick
 * composer keystrokes are captured as the initial prompt. The caller
 * owns error surfacing (toast) — this helper only reports the result.
 */
export async function launchDraftWithPreset(
  draftId: DraftId,
  preset: TerminalPreset,
): Promise<MaterializeResult> {
  const state = useChatDraftStore.getState();
  const draft = state.draftsById[draftId];
  if (!draft) return { success: false, error: "Draft no longer exists" };
  const chat = useAgentChatStore.getState();

  const skillBodies = resolveSkillBodies(
    draft.inputDraft,
    selectActiveSkills(useSkillsStore.getState()),
  );

  const result = await materializeWithPreset(
    draft,
    preset,
    draft.inputDraft,
    {
      markPromoting: state.markPromoting,
      markMaterialized: state.markMaterialized,
      markPromoted: state.markPromoted,
      markSendFailed: state.markSendFailed,
      ensureThread: chat.ensureThread,
      appendUserMessage: chat.appendUserMessage,
      removeUserMessageByNonce: chat.removeUserMessageByNonce,
      setModel: chat.setModel,
      setPermissionMode: chat.setPermissionMode,
      setSessionLaunchMode: chat.setSessionLaunchMode,
      setEffort: chat.setEffort,
      setContextWindow: chat.setContextWindow,
      setMode: chat.setMode,
    },
    skillBodies,
  );

  if (result.success) {
    const draftIdToClear = draft.draftId;
    state.setActiveDraft(null);
    setTimeout(
      () => useChatDraftStore.getState().clearDraft(draftIdToClear),
      CLEAR_AFTER_PROMOTION_MS,
    );
  }

  return result;
}
