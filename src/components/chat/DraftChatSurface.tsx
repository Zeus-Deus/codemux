import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { materializeAndSend } from "@/lib/agent-chat/materialize";
import { hasUltrathinkInBodyText } from "@/lib/agent-chat/ultrathink";
import { toast } from "@/lib/toast";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useAppStore } from "@/stores/app-store";
import {
  selectActiveDraft,
  useChatDraftStore,
  type ChatDraft,
} from "@/stores/chat-draft-store";
import {
  selectCapabilities,
  selectModel,
  useProviderCapabilities,
} from "@/stores/provider-capabilities-store";
import {
  activateWorkspace,
  agentChatCreatePane,
  getHomeDir,
} from "@/tauri/commands";

import { ChatHomeLanding } from "./ChatHomeLanding";
import { Composer } from "./Composer";
import { DEFAULT_THREAD_PERMISSION_MODE } from "@/stores/agent-chat-store";
import { ProjectPicker } from "@/components/overlays/project-picker";
import { DerivativeBranchPicker } from "./pickers/DerivativeBranchPicker";
import { WorktreePicker } from "./pickers/WorktreePicker";

/** Grace period between `markPromoted` and `clearDraft`. Gives any
 *  in-flight selector a chance to observe the promotion before the
 *  draft is swept. Matches the window called out in §9 of the plan. */
const CLEAR_AFTER_PROMOTION_MS = 5000;

/**
 * Top-level draft surface. Reads the active draft, wires a Composer
 * against it, and promotes the draft via `materializeAndSend` on the
 * first message send.
 */
export function DraftChatSurface() {
  const draft = useChatDraftStore(selectActiveDraft);
  if (!draft) return null;
  // Key on the draft id so a slot swap (home → project) remounts with a
  // fresh resolved-cwd effect instead of carrying stale state.
  return <DraftChatSurfaceInner key={draft.draftId} draft={draft} />;
}

function DraftChatSurfaceInner({ draft }: { draft: ChatDraft }) {
  const capabilities = useProviderCapabilities((s) =>
    selectCapabilities(s, draft.provider),
  );
  const activeModel = selectModel(capabilities, draft.model);
  const effortLabelMap = capabilities?.effort_label_map ?? {};
  const permissionModes = capabilities?.permission_modes ?? null;

  const updateDraftInput = useChatDraftStore((s) => s.updateDraftInput);
  const updateDraftConfig = useChatDraftStore((s) => s.updateDraftConfig);
  const updateDraftTarget = useChatDraftStore((s) => s.updateDraftTarget);
  const setActiveDraft = useChatDraftStore((s) => s.setActiveDraft);
  const clearDraft = useChatDraftStore((s) => s.clearDraft);

  // Resolve display cwd for Zone 1 of the composer. The Zone 1 slot
  // is now always overridden by a picker (ProjectPicker for home
  // drafts, WorktreePicker otherwise), so this only matters as a
  // fallback for the rare case where the override resolves to null.
  const [resolvedHomeDir, setResolvedHomeDir] = useState<string | null>(null);
  useEffect(() => {
    if (draft.target.kind !== "home") return;
    let cancelled = false;
    getHomeDir()
      .then((dir) => {
        if (!cancelled) setResolvedHomeDir(dir);
      })
      .catch((err) => {
        console.warn("[draft-chat-surface] getHomeDir failed:", err);
      });
    return () => {
      cancelled = true;
    };
  }, [draft.target.kind]);

  const existingWorkspaceCwd = useAppStore((s) => {
    if (draft.target.kind !== "existing_workspace") return null;
    const wsId = draft.target.workspaceId;
    return s.appState?.workspaces.find((w) => w.workspace_id === wsId)?.cwd ?? null;
  });

  // Project root of the workspace targeted by an `existing_workspace`
  // draft. Used to scope the WorktreePicker (so the dropdown lists the
  // sibling worktrees of the targeted workspace, not unrelated ones).
  const existingWorkspaceProjectRoot = useAppStore((s) => {
    if (draft.target.kind !== "existing_workspace") return null;
    const wsId = draft.target.workspaceId;
    const ws = s.appState?.workspaces.find((w) => w.workspace_id === wsId);
    return ws?.project_root ?? ws?.cwd ?? null;
  });

  // Derivative branch for "+ New worktree…" inline submit. Lives at
  // this level (not the picker) so WorktreePicker and
  // DerivativeBranchPicker stay in sync.
  const [derivativeBranch, setDerivativeBranch] = useState("main");

  const displayCwd = useMemo(() => {
    switch (draft.target.kind) {
      case "home":
        return null; // ProjectPicker zone1Override owns this slot
      case "project":
        return draft.target.projectPath;
      case "existing_workspace":
        return existingWorkspaceCwd;
    }
  }, [draft.target, existingWorkspaceCwd]);

  // Synchronous guard against duplicate submissions while a single
  // materialise is in flight. Mirrors AgentChatPane's send-in-flight
  // ref pattern.
  const sendInFlightRef = useRef(false);

  const handleSubmit = useCallback(() => {
    if (sendInFlightRef.current) return;
    // Re-read fresh state so a same-tick keystroke then Enter sees the
    // just-written text.
    const state = useChatDraftStore.getState();
    const currentDraft = state.draftsById[draft.draftId];
    if (!currentDraft) return;
    if (currentDraft.promoting) return;
    const text = currentDraft.inputDraft.trim();
    if (!text) return;

    // Resolve the cwd the backend should launch under.
    const cwdForSession = (() => {
      switch (currentDraft.target.kind) {
        case "home":
          return resolvedHomeDir;
        case "project":
          return currentDraft.target.projectPath;
        case "existing_workspace":
          return existingWorkspaceCwd;
      }
    })();

    if (!cwdForSession) {
      toast.error("Unable to resolve working directory for this chat.");
      return;
    }

    sendInFlightRef.current = true;
    const chat = useAgentChatStore.getState();
    void materializeAndSend(currentDraft, text, cwdForSession, {
      markPromoting: state.markPromoting,
      markPromoted: state.markPromoted,
      markSendFailed: state.markSendFailed,
      ensureThread: chat.ensureThread,
      appendUserMessage: chat.appendUserMessage,
      setModel: chat.setModel,
      setPermissionMode: chat.setPermissionMode,
      setSessionLaunchMode: chat.setSessionLaunchMode,
      setEffort: chat.setEffort,
      setContextWindow: chat.setContextWindow,
    })
      .then((result) => {
        if (result.success) {
          setActiveDraft(null);
          // §9: keep the draft entry for a grace period so any in-
          // flight UI can still observe the promotion before cleanup.
          const draftIdToClear = currentDraft.draftId;
          setTimeout(() => clearDraft(draftIdToClear), CLEAR_AFTER_PROMOTION_MS);
        } else {
          toast.error(`Send failed: ${result.error}`);
        }
      })
      .finally(() => {
        sendInFlightRef.current = false;
      });
  }, [
    draft.draftId,
    resolvedHomeDir,
    existingWorkspaceCwd,
    setActiveDraft,
    clearDraft,
  ]);

  // The textarea onStop is a no-op during a draft — there's no session
  // to interrupt. The button only shows when streaming=true, which we
  // set only during the in-flight materialise window. Stopping
  // mid-materialise would leave orphan state on the backend; we
  // intentionally don't wire it.
  const handleStop = useCallback(() => {
    // No-op — see comment above.
  }, []);

  // Provider change during a draft is cheap because nothing is live —
  // just update the draft's recorded provider. A side effect: the
  // permission-mode default differs between providers, so we seed a
  // sensible default whenever the provider flips.
  const handleProviderChange = useCallback(
    (next: ChatDraft["provider"]) => {
      if (next === draft.provider) return;
      updateDraftConfig(draft.draftId, {
        provider: next,
        // Reset provider-specific config to null so pickers can
        // re-seed from capabilities on next render.
        model: null,
        permissionMode:
          next === "claude" ? DEFAULT_THREAD_PERMISSION_MODE : null,
        effort: null,
        contextWindow: null,
      });
    },
    [draft.draftId, draft.provider, updateDraftConfig],
  );

  const handleModelChange = useCallback(
    (next: string) => updateDraftConfig(draft.draftId, { model: next }),
    [draft.draftId, updateDraftConfig],
  );
  const handlePermissionModeChange = useCallback(
    (next: string) =>
      updateDraftConfig(draft.draftId, { permissionMode: next }),
    [draft.draftId, updateDraftConfig],
  );
  const handleEffortChange = useCallback(
    (next: string) => updateDraftConfig(draft.draftId, { effort: next }),
    [draft.draftId, updateDraftConfig],
  );
  const handleContextWindowChange = useCallback(
    (next: string) =>
      updateDraftConfig(draft.draftId, { contextWindow: next }),
    [draft.draftId, updateDraftConfig],
  );

  // Zone 1 dispatch:
  //  - home target → ProjectPicker (lets the user retarget without
  //    sending first; clicking a project switches the draft target).
  //  - project target → WorktreePicker + DerivativeBranchPicker scoped
  //    to that project. The "+ New worktree…" row transforms into an
  //    inline input that creates a worktree directly (no dialog). The
  //    derivative picker controls the base branch for that submit.
  //  - existing_workspace target → same pair scoped to the targeted
  //    workspace's project.
  const handleWorktreeCreated = async (wsId: string) => {
    // Spawn an agent_chat pane into the fresh workspace BEFORE
    // activating. A worktree created via `createWorktreeWorkspace`
    // with no agent preset is a workspace with zero panes; if we
    // activate it as-is, `useEnsureDraftWhenEmpty` detects the empty
    // surface and auto-creates a Home draft, which then overrides
    // the workspace view. Mirrors the `launchChatAgentOnWorkspace`
    // path used by the preset bar.
    try {
      await agentChatCreatePane(wsId, "claude", null);
    } catch (err) {
      console.error("Failed to create agent_chat pane for new worktree:", err);
    }
    // Clear the draft after the pane exists so the ensure-draft hook
    // has a non-empty workspace to observe and stays dormant.
    setActiveDraft(null);
    activateWorkspace(wsId).catch(console.error);
  };

  const zone1Override = (() => {
    if (draft.target.kind === "home") {
      return (
        <ProjectPicker
          value={null}
          onChange={(path) => {
            updateDraftTarget(draft.draftId, {
              kind: "project",
              projectPath: path,
            });
          }}
        />
      );
    }
    if (draft.target.kind === "project") {
      const projectPath = draft.target.projectPath;
      return (
        <div className="flex items-center gap-2">
          <WorktreePicker
            mode="draft"
            projectPath={projectPath}
            draftTarget={draft.target}
            derivativeBranch={derivativeBranch}
            onChangeDraftTarget={(target) =>
              updateDraftTarget(draft.draftId, target)
            }
            onWorktreeCreated={handleWorktreeCreated}
          />
          <DerivativeBranchPicker
            projectPath={projectPath}
            value={derivativeBranch}
            onChange={setDerivativeBranch}
          />
        </div>
      );
    }
    if (draft.target.kind === "existing_workspace") {
      if (!existingWorkspaceProjectRoot) return null;
      return (
        <div className="flex items-center gap-2">
          <WorktreePicker
            mode="draft"
            projectPath={existingWorkspaceProjectRoot}
            draftTarget={draft.target}
            derivativeBranch={derivativeBranch}
            onChangeDraftTarget={(target) =>
              updateDraftTarget(draft.draftId, target)
            }
            onWorktreeCreated={handleWorktreeCreated}
          />
          <DerivativeBranchPicker
            projectPath={existingWorkspaceProjectRoot}
            value={derivativeBranch}
            onChange={setDerivativeBranch}
          />
        </div>
      );
    }
    return null;
  })();

  const composerEl = (
    <Composer
      draft={draft.inputDraft}
      cwd={displayCwd}
      provider={draft.provider}
      model={draft.model}
      permissionMode={draft.permissionMode}
      effort={draft.effort}
      contextWindow={draft.contextWindow}
      activeModel={activeModel}
      effortLabelMap={effortLabelMap}
      permissionModes={permissionModes}
      ultrathinkInBodyText={hasUltrathinkInBodyText(draft.inputDraft)}
      // While promoting, treat the composer like a streaming chat so
      // the submit button disables and a spinner could be added later.
      streaming={draft.promoting}
      // Drafts do not need a backend session to be considered ready —
      // Enter-to-send is always available as long as text is present.
      sessionReady={true}
      showProviderPicker={false}
      showStopButton={false}
      errorMessage={draft.lastSendError}
      zone1Override={zone1Override}
      onDraftChange={(next) => updateDraftInput(draft.draftId, next)}
      onSubmit={handleSubmit}
      onStop={handleStop}
      onProviderChange={handleProviderChange}
      onModelChange={handleModelChange}
      onPermissionModeChange={handlePermissionModeChange}
      onEffortChange={handleEffortChange}
      onContextWindowChange={handleContextWindowChange}
    />
  );

  return (
    <div className="flex h-full w-full flex-col bg-background">
      <ChatHomeLanding composer={composerEl} />
    </div>
  );
}
