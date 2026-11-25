import { materializeWithPreset } from "./materialize";
import type { MaterializeResult } from "./materialize";
import { resolveSkillSelection, skillsForProvider } from "./skill-tokens";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useAppStore } from "@/stores/app-store";
import { useChatDraftStore, type DraftId } from "@/stores/chat-draft-store";
import { selectActiveSkills, useSkillsStore } from "@/stores/skills-store";
import { refreshSkillSelectionForCwd } from "./skill-selection-refresh";
import type { TerminalPreset } from "@/tauri/types";

/** Grace period between promoting a draft and sweeping its entry, so
 *  any in-flight UI can still observe the promotion before cleanup.
 *  Matches `CLEAR_AFTER_PROMOTION_MS` in DraftChatSurface. */
export const CLEAR_AFTER_PROMOTION_MS = 5000;

/**
 * Materialise an active draft by clicking a preset: commit the draft
 * (spawning only that preset's pane) via `materializeWithPreset`, then
 * show the live workspace. A CLI prompt withheld from terminal input stays
 * in its draft for the user to copy into the agent; empty drafts and native
 * chats still get the usual grace-period cleanup.
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
  const app = useAppStore.getState();
  const target = draft.target;
  const sourceCwd = (() => {
    switch (target.kind) {
      case "project":
        return target.projectPath;
      case "existing_workspace":
        return app.appState?.workspaces.find(
          (workspace) => workspace.workspace_id === target.workspaceId,
        )?.cwd ?? null;
      case "home":
        return app.homeDir;
    }
  })();
  const sourceSkills = selectActiveSkills(useSkillsStore.getState(), sourceCwd);

  const skillSelection = resolveSkillSelection(
    draft.inputDraft,
    skillsForProvider(sourceSkills, draft.provider),
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
      markManualPromptRequired: state.markManualPromptRequired,
      ensureThread: chat.ensureThread,
      appendUserMessage: chat.appendUserMessage,
      removeUserMessageByNonce: chat.removeUserMessageByNonce,
      setModel: chat.setModel,
      setPermissionMode: chat.setPermissionMode,
      setSessionLaunchMode: chat.setSessionLaunchMode,
      setEffort: chat.setEffort,
      setContextWindow: chat.setContextWindow,
      setServiceTier: chat.setServiceTier,
      setMode: chat.setMode,
      refreshSkillSelection: (selection, effectiveCwd) =>
        refreshSkillSelectionForCwd(
          selection,
          sourceSkills,
          sourceCwd ?? effectiveCwd,
          effectiveCwd,
          draft.provider,
        ),
      getIncludePlugins: () => useSkillsStore.getState().includePlugins,
    },
    skillSelection,
  );

  if (result.success) {
    state.setActiveDraft(null);
    if (!result.manualPromptRequired) {
      const draftIdToClear = draft.draftId;
      setTimeout(
        () => useChatDraftStore.getState().clearDraft(draftIdToClear),
        CLEAR_AFTER_PROMOTION_MS,
      );
    }
  }

  return result;
}
