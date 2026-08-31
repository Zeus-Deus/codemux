import { activateWorkspace } from "@/tauri/commands";
import { useAppStore, selectActiveWorkspaceId } from "@/stores/app-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import {
  abandonInteraction,
  beginInteraction,
  mark,
  NO_INTERACTION,
} from "./interaction-trace";

/** How long an unconfirmed optimistic selection may survive. The backend
 *  normally confirms within a frame or two; this only catches an activate
 *  that resolved without ever producing a matching snapshot, so selection
 *  can't stay wedged on a workspace the backend disagrees about. */
const PENDING_ACTIVATION_TIMEOUT_MS = 5_000;

let pendingTimeout: ReturnType<typeof setTimeout> | null = null;

/** Monotonic id of the most recent activation. The pending-id check alone
 *  cannot tell two rapid activations of the SAME workspace apart, so an early
 *  call's rejection could disarm the newer call's timeout and restore a draft
 *  view over its still-in-flight switch. Only the newest call may roll back. */
let activationSeq = 0;

function armPendingTimeout(workspaceId: string): void {
  if (pendingTimeout !== null) clearTimeout(pendingTimeout);
  pendingTimeout = setTimeout(() => {
    pendingTimeout = null;
    useAppStore.getState().clearPendingActivation(workspaceId);
  }, PENDING_ACTIVATION_TIMEOUT_MS);
}

function disarmPendingTimeout(): void {
  if (pendingTimeout === null) return;
  clearTimeout(pendingTimeout);
  pendingTimeout = null;
}

/**
 * The one workspace-activation path. Every surface that navigates to a
 * workspace routes through here, so selection is optimistic, traced and
 * draft-clearing everywhere instead of only on the sidebar.
 *
 * Order matters: the pending id
 * is written synchronously, in the click's own task, before the IPC is fired.
 * That single store write is what paints — the sidebar highlight flips and
 * `WorkspaceMain`/`PaneContainer` mount the target pane from workspace data
 * the renderer already holds, without waiting for the snapshot round trip.
 * Selection therefore stays urgent work: no `startTransition` wrapper (the
 * old one deferred nothing, since the callback performed no state update).
 *
 * Returns the invoke promise so callers can sequence work after the backend
 * has actually switched. It REJECTS on failure — the optimistic selection has
 * already been rolled back by then, and fire-and-forget callers are expected
 * to attach their own `.catch`, exactly as they did with the raw command.
 */
export function activateWorkspaceInteraction(workspaceId: string): Promise<void> {
  const draftStore = useChatDraftStore.getState();
  // Capture the draft view this activation dismisses so a rejected invoke can
  // restore it along with the workspace selection. Only the active pointer is
  // cleared here — the draft body stays in `draftsById` either way.
  const dismissedDraftId = draftStore.activeDraftId;
  draftStore.setActiveDraft(null);
  // Re-selecting the workspace already on screen mounts no new pane tree, so
  // `PaneContainer`'s keyed layout effect never re-runs and the trace would
  // have no paint to close on — it would just sit open collecting long tasks
  // that belong to whatever the user does next.
  const repaints = selectActiveWorkspaceId(useAppStore.getState()) !== workspaceId;
  const seq = ++activationSeq;
  useAppStore.getState().beginPendingActivation(workspaceId);
  armPendingTimeout(workspaceId);
  const interaction = repaints
    ? beginInteraction("workspace-switch", { target: workspaceId })
    : NO_INTERACTION;
  mark(interaction, "click");
  mark(interaction, "invoke-start");
  return activateWorkspace(workspaceId).then(
    () => {
      mark(interaction, "invoke-returned");
    },
    (error: unknown) => {
      // Rollback: the backend never switched, so selection must snap back to
      // whatever the last snapshot said. Scoped to our own id AND to being the
      // newest activation, so a late rejection can't cancel a newer selection
      // (or disarm its timeout) — including a newer in-flight activation of
      // the very same workspace.
      const store = useAppStore.getState();
      if (seq === activationSeq && store.pendingActiveWorkspaceId === workspaceId) {
        disarmPendingTimeout();
        store.clearPendingActivation(workspaceId);
        // The rollback lands the user back on the pre-activation view, so the
        // draft surface this activation dismissed must come back with it.
        // Scoped exactly like the selection rollback above: a newer
        // activation re-clears the pointer under its own id (and flips the
        // pending id, so we never reach here), and a draft the user opened
        // since — or one that no longer exists — is left alone.
        const drafts = useChatDraftStore.getState();
        if (
          dismissedDraftId !== null &&
          drafts.activeDraftId === null &&
          drafts.draftsById[dismissedDraftId] !== undefined
        ) {
          drafts.setActiveDraft(dismissedDraftId);
        }
      }
      // A rejected activate never produces a snapshot, so the trace would
      // otherwise sit open until the abandon timeout.
      abandonInteraction(interaction);
      throw error;
    },
  );
}
