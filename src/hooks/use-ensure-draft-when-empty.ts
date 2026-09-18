import { useEffect, useRef } from "react";
import { selectActiveWorkspaceId, useAppStore, useHomeDir } from "@/stores/app-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { hasAnyPane } from "@/lib/pane-tree";
import { launchAgentChatPane } from "@/lib/agent-chat/launch-pane";
import {
  adoptLocalFallbackActivation,
  noteLocalWorkspaceActivation,
  wasActivatedLocally,
} from "@/lib/local-activation";

/**
 * Primitive-summary selector for the bits of `appState` this hook
 * actually depends on. Returning a string fingerprint means the effect
 * below only re-runs when one of the four relevant fields changes —
 * not on every backend `app-state-changed` tick (agent tokens, git
 * polls, hook events). Without this, the effect body would walk the
 * surface tree on every tick under heavy load.
 */
function selectEmptyWorkspaceFingerprint(
  s: Parameters<typeof selectActiveWorkspaceId>[0],
): string {
  const app = s.appState;
  if (!app) return "no-app-state";
  const wsId = selectActiveWorkspaceId(s);
  if (!wsId) return "no-active-ws";
  const ws = app.workspaces.find((w) => w.workspace_id === wsId);
  if (!ws) return `ws-missing:${wsId}`;
  // Whether the active surface has any panes — this is the only
  // structural fact the effect cares about.
  const activeSurface = ws.surfaces.find(
    (sf) => sf.surface_id === ws.active_surface_id,
  );
  const hasPane = activeSurface ? hasAnyPane(activeSurface.root) : false;
  // Project_root vs cwd drives the Home-draft-vs-pane branch.
  const projectRoot = ws.project_root ?? ws.cwd;
  return `${wsId}|${hasPane ? "1" : "0"}|${projectRoot}`;
}

/**
 * Ensure something is showing for the active workspace whenever the
 * user has nothing else to look at.
 *
 * Three-way dispatch when no draft is active and the active workspace
 * (or lack thereof) has no real panes to resume:
 *   - No active workspace → auto-create a Home draft.
 *   - Active workspace is Home-rooted (`project_root === homeDir`) →
 *     auto-create a Home draft (so "What should we do today?" replaces
 *     the empty-workspace splash).
 *   - Active workspace is a project workspace → auto-spawn an
 *     `agent_chat` pane instead. Dropping the user onto a Home draft
 *     that doesn't belong to this workspace would be confusing and
 *     breaks "messages go to the workspace I clicked."
 *
 * Runs only when `enable_agent_chat` + `enable_lazy_workspace_creation`
 * are both on (and flags have loaded). Idempotent:
 *  - Once `activeDraftId` becomes non-null, the early return up top
 *    stops the effect from running again until the draft clears.
 *  - The pane-spawn branch guards with an in-flight ref so repeated
 *    effect fires while the Tauri call is pending don't double-spawn.
 *  - The pane-spawn branch additionally requires that THIS client
 *    activated the workspace (`wasActivatedLocally`). The in-flight ref
 *    is client-local. Selection is also local for remote clients, but a
 *    newly connected client can seed from a workspace another client is
 *    still populating, so the explicit-navigation check remains useful. The Home-draft branch needs no such check — a
 *    draft is purely client-local state.
 */
export function useEnsureDraftWhenEmpty() {
  // Subscribe to a primitive string fingerprint of the four fields
  // this effect actually depends on, NOT the whole appState. Without
  // this, the effect body re-ran on every backend `app-state-changed`
  // tick (many per second under load) walking surface trees and doing
  // workspace lookups for nothing. The audit pass found this pattern;
  // see `selectEmptyWorkspaceFingerprint` above.
  const fingerprint = useAppStore(selectEmptyWorkspaceFingerprint);
  const homeDir = useHomeDir();
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);
  const enableLazyWorkspaceCreation = useFeatureFlags(
    (s) => s.enableLazyWorkspaceCreation,
  );
  const flagsLoaded = useFeatureFlags((s) => s.loaded);
  const activeDraftId = useChatDraftStore((s) => s.activeDraftId);

  // Tracks the workspace_id we most recently kicked off an
  // `launchAgentChatPane` call for, so rapid state updates don't
  // fan out into multiple pane-spawn requests for the same empty
  // workspace. Cleared on failure so a retry can fire.
  const inFlightSpawnRef = useRef<string | null>(null);

  // Boot restore has no activation to observe: the backend hands us a
  // workspace that is already active, nobody clicked anything, and that
  // workspace still needs its pane. Adopt whatever is active the first
  // time this effect reaches a real snapshot, and require a local
  // activation for every workspace after it.
  //
  // That leaves one narrow window: a client booting at the exact moment
  // another client creates and activates a workspace adopts it as its
  // own. It stays narrow because the `activeDraftId` early return above
  // sits ahead of this adoption, and because it only spans the first
  // snapshot this client ever sees — afterwards the local-activation
  // record is the only way in.
  const bootAdoptedRef = useRef(false);

  useEffect(() => {
    if (!flagsLoaded) return;
    if (!enableAgentChat || !enableLazyWorkspaceCreation) return;
    if (activeDraftId) return;

    // Re-read the live snapshot at effect-fire time. The fingerprint
    // dep above ensures we only get here when the relevant slice
    // changed, but we still need the full workspace object to read
    // surfaces / surface ids.
    const storeState = useAppStore.getState();
    const appState = storeState.appState;
    const activeWorkspaceId = selectActiveWorkspaceId(storeState);
    if (!appState) return;

    if (!bootAdoptedRef.current) {
      bootAdoptedRef.current = true;
      if (activeWorkspaceId) {
        noteLocalWorkspaceActivation(activeWorkspaceId);
      }
    }

    // Closing or archiving a workspace from THIS client hands the backend
    // the job of picking the next active one, so there is no activation to
    // observe — only the marker the close/archive command left behind. The
    // workspace that fallback lands on is ours to fill like any other we
    // navigated to. Consumed on this first snapshot after the command no
    // matter where we landed, so it never outlives the fallback it marks.
    adoptLocalFallbackActivation(activeWorkspaceId ?? "");

    const activeWs = appState.workspaces.find(
      (w) => w.workspace_id === activeWorkspaceId,
    );
    if (activeWs) {
      const activeSurface = activeWs.surfaces.find(
        (s) => s.surface_id === activeWs.active_surface_id,
      );
      if (activeSurface && hasAnyPane(activeSurface.root)) return;

      // Empty workspace. If we can confidently say this is a project
      // workspace (not Home-rooted), inject an agent_chat pane rather
      // than falling back to the Home draft — the user clicked a
      // specific workspace, their next message should land on it.
      // When `homeDir` is still null (boot hasn't resolved it yet) we
      // defer to the legacy Home-draft path for safety.
      const isProjectWorkspace =
        homeDir !== null &&
        (activeWs.project_root ?? activeWs.cwd) !== homeDir;
      if (isProjectWorkspace) {
        // Another client put this workspace on screen — it owns filling
        // it. Injecting a pane from here would duplicate whatever that
        // client is already launching. Return without the Home draft
        // either: the workspace on screen is a project workspace, and
        // the client that activated it is about to populate it.
        if (!wasActivatedLocally(activeWs.workspace_id)) return;
        if (inFlightSpawnRef.current === activeWs.workspace_id) return;
        inFlightSpawnRef.current = activeWs.workspace_id;
        launchAgentChatPane(activeWs.workspace_id, "claude", null).catch(
          (err) => {
            console.error(
              "[ensure-draft] failed to auto-spawn agent_chat pane:",
              err,
            );
            inFlightSpawnRef.current = null;
          },
        );
        return;
      }
    }

    const store = useChatDraftStore.getState();
    const draft = store.getOrCreateHomeDraft();
    store.setActiveDraft(draft.draftId);
  }, [
    fingerprint,
    homeDir,
    enableAgentChat,
    enableLazyWorkspaceCreation,
    flagsLoaded,
    activeDraftId,
  ]);
}
