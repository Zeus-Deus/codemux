import { useState } from "react";
import {
  ContextMenu,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import {
  WorkspaceContextMenuItems,
  DeleteWorktreeDialog,
} from "./sidebar-workspace-row";
import { ConfirmPushDialog } from "@/components/overlays/confirm-push-dialog";
import {
  archiveWorkspace,
  closeWorkspace,
  closeWorkspaceWithWorktree,
  unarchiveWorkspace,
  workspacePullBack,
  workspacePushToHost,
  type HostView,
} from "@/tauri/commands";
import { toast } from "@/lib/toast";
import type { WorkspaceSnapshot } from "@/tauri/types";
import type {
  SettleMenuAction,
  SnoozeMenuAction,
} from "./sidebar-workspace-row";

interface Props {
  workspace: WorkspaceSnapshot;
  /** Optional Settle / Un-settle entry surfaced at the top of the menu —
   *  the inbox card passes "settle" (when settleable), a settled row passes
   *  "unsettle", so the right-click menu mirrors the hover affordance. */
  settleAction?: SettleMenuAction;
  /** Optional Snooze / Wake entry, the deferral sibling of `settleAction` —
   *  a card passes "snooze" with its resolved presets, a snoozed row passes
   *  "wake". */
  snoozeAction?: SnoozeMenuAction;
  /** Optional "Mark unread" entry. */
  unreadAction?: { onMarkUnread: () => void };
  children: React.ReactNode;
}

/** Shared right-click wrapper for both inbox row shapes (active card and
 *  settled one-liner): the standard workspace context menu plus the
 *  destructive-delete and push-confirm dialogs it can open. Owns the same
 *  archive/close/push semantics as everywhere else — local rows archive
 *  (undoable), attach-in-place/remote rows close (the backend refuses to
 *  archive them). */
export function WorkspaceInboxMenu({
  workspace,
  settleAction,
  snoozeAction,
  unreadAction,
  children,
}: Props) {
  const [showDeleteDialog, setShowDeleteDialog] = useState(false);
  const [pendingPushHost, setPendingPushHost] = useState<HostView | null>(null);

  const isAttachOrRemote =
    workspace.attach_only === true || workspace.host_id != null;
  const isPrimary = !workspace.worktree_path;
  const canDelete =
    !isPrimary && workspace.protected !== true && !isAttachOrRemote;

  const handleArchiveOrClose = async () => {
    try {
      if (isAttachOrRemote) {
        if (workspace.worktree_path) {
          await closeWorkspaceWithWorktree(
            workspace.workspace_id,
            false,
            false,
            false,
          );
        } else {
          await closeWorkspace(workspace.workspace_id, false);
        }
        toast.success(
          `Closed "${workspace.title}" — it stays available on its host in the Workspaces Overview`,
        );
      } else {
        const archiveId = await archiveWorkspace(workspace.workspace_id);
        toast.undoable({
          message: `Archived "${workspace.title}"`,
          description: "Restore anytime from Settings → Archive.",
          onUndo: async () => {
            await unarchiveWorkspace(archiveId);
          },
        });
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      toast.error(isAttachOrRemote ? "Close failed" : "Archive failed", {
        description: message,
      });
    }
  };

  const performPushToHost = async (host: HostView) => {
    try {
      const result = await workspacePushToHost(workspace.workspace_id, host.id);
      if (result.ok) {
        toast.undoable({
          message: `Pushed to ${host.name}`,
          description: "Tap Undo within 10s to pull it back.",
          onUndo: async () => {
            const undoResult = await workspacePullBack(workspace.workspace_id);
            if (undoResult.ok) {
              toast.success(`Pulled back from ${host.name}`);
            } else {
              toast.error("Pull back failed", {
                description: undoResult.message,
              });
            }
          },
        });
      } else {
        toast.error(`Push to ${host.name} failed`, {
          description: result.message,
        });
      }
    } catch (err) {
      toast.error("Push failed", {
        description: err instanceof Error ? err.message : String(err),
      });
    }
  };

  return (
    <>
      <ContextMenu>
        <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
        <WorkspaceContextMenuItems
          workspace={workspace}
          settleAction={settleAction}
          snoozeAction={snoozeAction}
          unreadAction={unreadAction}
          onArchiveRequest={() => void handleArchiveOrClose()}
          onDeleteRequest={() => setShowDeleteDialog(true)}
          onRequestPushConfirm={(host) => setPendingPushHost(host)}
        />
      </ContextMenu>
      {canDelete && (
        <DeleteWorktreeDialog
          workspace={workspace}
          open={showDeleteDialog}
          onOpenChange={setShowDeleteDialog}
        />
      )}
      <ConfirmPushDialog
        open={pendingPushHost !== null}
        workspaceTitle={workspace.title}
        host={pendingPushHost}
        onConfirm={() => {
          if (pendingPushHost) void performPushToHost(pendingPushHost);
        }}
        onOpenChange={(open) => {
          if (!open) setPendingPushHost(null);
        }}
      />
    </>
  );
}
