import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { materializeAndSend } from "@/lib/agent-chat/materialize";
import { prestartWorktreeSession } from "@/lib/agent-chat/prestart-worktree-session";
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
import { activateWorkspace } from "@/tauri/commands";

import { ChatHomeLanding } from "./ChatHomeLanding";
import { Composer } from "./Composer";
import { DEFAULT_THREAD_PERMISSION_MODE } from "@/stores/agent-chat-store";
import { ProjectPicker } from "@/components/overlays/project-picker";
import { DerivativeBranchPicker } from "./pickers/DerivativeBranchPicker";
import type { ActivePillMode } from "./pickers/ModePill";
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

  // Home directory is hydrated once at App mount into useAppStore.
  // Reading it from there (rather than re-fetching via a local Tauri
  // roundtrip on every DraftChatSurface mount) closes a race where
  // the user hit Send before the roundtrip resolved: the seed effect
  // below would still be blocked on its gate, `draft.target.kind`
  // stayed on "home", and materializeAndSend went down the
  // `createHomeRootedWorkspace` path — minting a duplicate workspace
  // instead of reusing the active sidebar workspace.
  const appHomeDir = useAppStore((s) => s.homeDir);

  const existingWorkspaceCwd = useAppStore((s) => {
    if (draft.target.kind !== "existing_workspace") return null;
    const wsId = draft.target.workspaceId;
    return s.appState?.workspaces.find((w) => w.workspace_id === wsId)?.cwd ?? null;
  });

  // Identity + project path of the currently active sidebar workspace.
  // Split into two primitive selectors (instead of a single object
  // selector) so Zustand's default Object.is equality doesn't see a
  // fresh reference every render and retrigger the seed effect in an
  // infinite loop. Used to seed a home draft's target when the user
  // lands on Home while a project workspace is active, so the composer
  // mirrors the sidebar context instead of sitting on "Select project."
  // We seed to `existing_workspace` (not `project`) because the user's
  // intent is "send into the active workspace I'm looking at," not
  // "spawn a new worktree for this project" — the latter would double
  // the workspace on submit.
  const activeSidebarWorkspaceId = useAppStore((s) => {
    const st = s.appState;
    if (!st?.active_workspace_id) return null;
    const ws = st.workspaces.find(
      (w) => w.workspace_id === st.active_workspace_id,
    );
    return ws ? ws.workspace_id : null;
  });
  const activeSidebarProjectPath = useAppStore((s) => {
    const st = s.appState;
    if (!st?.active_workspace_id) return null;
    const ws = st.workspaces.find(
      (w) => w.workspace_id === st.active_workspace_id,
    );
    return ws ? ws.project_root ?? ws.cwd ?? null : null;
  });

  // Seed the home draft's target with the active sidebar workspace on
  // mount. Gated on appHomeDir so a home-rooted workspace (where
  // `project_root === $HOME`) stays on the general Home landing rather
  // than being mistaken for a real project. Runs once per draft: as
  // soon as the target flips away from "home" the guard short-circuits
  // every subsequent render, so the user's later picker changes stick.
  useEffect(() => {
    if (draft.target.kind !== "home") return;
    if (appHomeDir === null) return;
    if (!activeSidebarWorkspaceId || !activeSidebarProjectPath) return;
    if (activeSidebarProjectPath === appHomeDir) return;
    updateDraftTarget(draft.draftId, {
      kind: "existing_workspace",
      workspaceId: activeSidebarWorkspaceId,
    });
  }, [
    draft.target.kind,
    draft.draftId,
    activeSidebarWorkspaceId,
    activeSidebarProjectPath,
    appHomeDir,
    updateDraftTarget,
  ]);

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
    let currentDraft = state.draftsById[draft.draftId];
    if (!currentDraft) return;
    if (currentDraft.promoting) return;
    const text = currentDraft.inputDraft.trim();
    if (!text) return;

    // Submit-time salvage: if we're about to send a "home" draft while
    // a project-rooted sidebar workspace is active, silently re-route
    // the target to `existing_workspace` before handing off to
    // materialize. The mount-time seed effect normally catches this,
    // but if the user types-and-sends fast enough that the effect
    // hasn't run yet (React `useEffect` runs post-paint, and the
    // appHomeDir selector might still be null on the very first tick)
    // we'd otherwise hit `createHomeRootedWorkspace` and mint a
    // duplicate workspace.
    if (
      currentDraft.target.kind === "home" &&
      activeSidebarWorkspaceId !== null &&
      activeSidebarProjectPath !== null &&
      appHomeDir !== null &&
      activeSidebarProjectPath !== appHomeDir
    ) {
      state.updateDraftTarget(currentDraft.draftId, {
        kind: "existing_workspace",
        workspaceId: activeSidebarWorkspaceId,
      });
      // Re-read after the mutation so the snapshot handed to
      // materializeAndSend carries the corrected target.
      const refreshed = useChatDraftStore
        .getState()
        .draftsById[currentDraft.draftId];
      if (refreshed) currentDraft = refreshed;
    }

    // Resolve the cwd the backend should launch under.
    const cwdForSession = (() => {
      switch (currentDraft.target.kind) {
        case "home":
          return appHomeDir;
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
      setMode: chat.setMode,
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
    appHomeDir,
    existingWorkspaceCwd,
    activeSidebarWorkspaceId,
    activeSidebarProjectPath,
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

  // Drafts have no live session yet, so mode activation / removal
  // is state-only: we just stamp the new mode on the draft. On
  // materialize, `effectivePermissionMode` reads `draft.mode` and
  // overrides `permission_mode` for the `start_session` call (Plan
  // boots the SDK straight into plan mode — no restart needed
  // because there's no session to restart). Ask / Debug are
  // tracked at the slice level only; their prompt-wrappers land in
  // Stages 4 & 6.
  const handleModeActivate = useCallback(
    (newMode: ActivePillMode) => {
      updateDraftConfig(draft.draftId, { mode: newMode });
    },
    [draft.draftId, updateDraftConfig],
  );
  const handleModeRemove = useCallback(() => {
    updateDraftConfig(draft.draftId, { mode: "default" });
  }, [draft.draftId, updateDraftConfig]);

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
    // Attach a chat pane AND pre-start the provider session BEFORE
    // activating the workspace. This mirrors materializeAndSend's
    // ordering: by the time AgentChatPane mounts, `pane.thread_id`
    // is already set so the mount-effect adopts it via the
    // `if (threadId) ensureThread(...)` branch, avoiding the race
    // where the user's first send lands with an unregistered
    // thread_id → `session_not_found`. See
    // `prestart-worktree-session.ts` for the full rationale.
    try {
      await prestartWorktreeSession(wsId);
    } catch (err) {
      console.error("Failed to prestart worktree chat session:", err);
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
      mode={draft.mode}
      onDraftChange={(next) => updateDraftInput(draft.draftId, next)}
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
      <DraftSurfaceHeader />
      <div className="flex-1 min-h-0 overflow-hidden">
        <ChatHomeLanding composer={composerEl} />
      </div>
    </div>
  );
}

/**
 * Placeholder chrome that matches AgentChatPaneHeader's visual band
 * (h-7 border-b) so the draft surface doesn't look "naked" next to a
 * materialized pane. Drafts have no session yet, so the session
 * selector / split / close controls from the real pane header don't
 * apply — we only borrow the silhouette.
 */
function DraftSurfaceHeader() {
  return (
    <header
      className="flex h-7 shrink-0 items-center gap-1 border-b border-border/30 bg-background px-1.5"
      data-testid="draft-surface-header"
    >
      <span className="px-1.5 text-xs text-muted-foreground">Agent Chat</span>
    </header>
  );
}
