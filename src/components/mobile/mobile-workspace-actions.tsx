import { useState } from "react";
import type { WorkspaceSnapshot } from "@/tauri/types";
import {
  archiveWorkspace,
  closeWorkspace,
  renameWorkspace,
  setWorkspaceMuted,
  setWorkspacePinned,
  unarchiveWorkspace,
} from "@/tauri/commands";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { toast } from "@/lib/toast";

export function MobileWorkspaceActions({
  workspace,
  onDone,
}: {
  workspace: WorkspaceSnapshot;
  onDone: () => void;
}) {
  const [renaming, setRenaming] = useState(false);
  const [name, setName] = useState(workspace.title);
  const [busy, setBusy] = useState(false);
  const attached = workspace.attach_only || workspace.host_id != null;
  async function run(action: () => Promise<unknown>) {
    setBusy(true);
    try {
      await action();
      onDone();
    } catch (error) {
      toast.error("Could not update workspace", { description: String(error) });
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <div className="mobile-workspace-actions">
        <p className="px-2 text-label text-muted-foreground truncate">
          {workspace.title}
        </p>
        <button
          disabled={busy}
          onClick={() => {
            setName(workspace.title);
            setRenaming(true);
          }}
        >
          Rename workspace
        </button>
        <button
          disabled={busy}
          onClick={() =>
            void run(() =>
              setWorkspacePinned(workspace.workspace_id, !workspace.pinned_at),
            )
          }
        >
          {workspace.pinned_at ? "Unpin workspace" : "Pin workspace"}
        </button>
        <button
          disabled={busy}
          onClick={() =>
            void run(() =>
              setWorkspaceMuted(
                workspace.workspace_id,
                !workspace.notifications_muted,
              ),
            )
          }
        >
          {workspace.notifications_muted
            ? "Unmute workspace"
            : "Mute workspace"}
        </button>
        <button
          disabled={busy}
          onClick={() =>
            void run(async () => {
              if (attached) await closeWorkspace(workspace.workspace_id, false);
              else {
                const id = await archiveWorkspace(workspace.workspace_id);
                toast.undoable({
                  message: `Archived “${workspace.title}”`,
                  description:
                    "Files stay on your desktop. Restore from Settings → Archive.",
                  onUndo: async () => {
                    await unarchiveWorkspace(id);
                  },
                });
              }
            })
          }
        >
          {attached ? "Close workspace" : "Archive workspace"}
        </button>
      </div>
      <Dialog open={renaming} onOpenChange={setRenaming}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Rename workspace</DialogTitle>
            <DialogDescription>
              Choose a name you can find easily.
            </DialogDescription>
          </DialogHeader>
          <form
            className="flex flex-col gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              void run(async () => {
                await renameWorkspace(workspace.workspace_id, name.trim());
                setRenaming(false);
              });
            }}
          >
            <input
              className="mobile-search m-0 px-3 py-2"
              aria-label="Workspace name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              autoFocus
            />
            <button
              className="mobile-primary"
              disabled={busy || !name.trim()}
              type="submit"
            >
              Save name
            </button>
          </form>
        </DialogContent>
      </Dialog>
    </>
  );
}
