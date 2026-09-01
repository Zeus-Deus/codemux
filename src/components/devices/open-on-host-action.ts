import { toast } from "@/lib/toast";
import {
  activateWorkspace,
  workspaceOpenOnHost,
  type WorkspaceSyncView,
} from "@/tauri/commands";

/**
 * "Open on <host>": attach to a host workspace in place — no rsync, no local
 * copy. The backend creates (or re-activates) a local attach-in-place
 * workspace whose terminal runs on the host; we then land the user on it.
 *
 * `onOpened` runs after activation succeeds so the caller can dismiss the
 * Devices page. Failures surface as a toast; the promise never rejects.
 */
export async function runOpenOnHost(
  row: WorkspaceSyncView,
  hostName: string,
  onOpened: () => void,
): Promise<void> {
  try {
    const result = await workspaceOpenOnHost(row.id);
    await activateWorkspace(result.workspace_id);
    onOpened();
    toast.success(`Opened on ${hostName}`, { description: result.message });
  } catch (err) {
    toast.error("Couldn't open on host", {
      description: err instanceof Error ? err.message : String(err),
    });
  }
}
