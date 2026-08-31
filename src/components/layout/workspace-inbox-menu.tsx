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
import { ProjectImageDialog } from "@/components/overlays/project-image-dialog";
import { ProjectAppearanceMenu } from "./project-appearance-menu";
import { useProjectAppearance } from "./use-project-appearance";
import { useProjectAppearanceStore } from "@/stores/project-appearance-store";
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
  /** The project this workspace belongs to (name + absolute root path).
   *  Carries the project-level avatar actions into the menu — every inbox row
   *  shape already resolves it for its avatar, so right-clicking any workspace
   *  can reach the settings for its owning project. */
  repo: { name: string; path: string };
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

function WorkspaceProjectImageDialog({
  repo,
  onOpenChange,
}: {
  repo: { name: string; path: string };
  onOpenChange: (open: boolean) => void;
}) {
  const { imageUrl } = useProjectAppearance(repo.path);
  const setProjectImage = useProjectAppearanceStore((state) => state.setImage);
  return (
    <ProjectImageDialog
      open
      onOpenChange={onOpenChange}
      projectName={repo.name}
      initialValue={imageUrl}
      onSave={(value) => setProjectImage(repo.path, value)}
    />
  );
}

/** Shared right-click wrapper for both inbox row shapes (active card and
 *  settled one-liner): the standard workspace context menu plus the
 *  destructive-delete and push-confirm dialogs it can open. Owns the same
 *  archive/close/push semantics as everywhere else — local rows archive
 *  (undoable), attach-in-place/remote rows close (the backend refuses to
 *  archive them). */
export function WorkspaceInboxMenu({
  workspace,
  repo,
  settleAction,
  snoozeAction,
  unreadAction,
  children,
}: Props) {
  const [contextMenuOpen, setContextMenuOpen] = useState(false);
  const [showDeleteDialog, setShowDeleteDialog] = useState(false);
  const [pendingPushHost, setPendingPushHost] = useState<HostView | null>(null);
  const [showImageDialog, setShowImageDialog] = useState(false);

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
          `Closed "${workspace.title}" — it stays available on its host under Devices`,
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
      <ContextMenu onOpenChange={setContextMenuOpen}>
        <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
        {contextMenuOpen && (
          <WorkspaceContextMenuItems
            workspace={workspace}
            project={repo}
            settleAction={settleAction}
            snoozeAction={snoozeAction}
            unreadAction={unreadAction}
            projectMenu={
              <ProjectAppearanceMenu
                projectName={repo.name}
                projectPath={repo.path}
                onRequestImageDialog={() => setShowImageDialog(true)}
              />
            }
            onArchiveRequest={() => void handleArchiveOrClose()}
            onDeleteRequest={() => setShowDeleteDialog(true)}
            onRequestPushConfirm={(host) => setPendingPushHost(host)}
          />
        )}
      </ContextMenu>

      {/* Sits outside the ContextMenu subtree: selecting the menu item
          unmounts the menu, which would tear the dialog down with it. */}
      {showImageDialog && (
        <WorkspaceProjectImageDialog
          repo={repo}
          onOpenChange={setShowImageDialog}
        />
      )}
      {canDelete && showDeleteDialog && (
        <DeleteWorktreeDialog
          workspace={workspace}
          open
          onOpenChange={setShowDeleteDialog}
        />
      )}
      {pendingPushHost && (
        <ConfirmPushDialog
          open
          workspaceTitle={workspace.title}
          host={pendingPushHost}
          onConfirm={() => void performPushToHost(pendingPushHost)}
          onOpenChange={(open) => {
            if (!open) setPendingPushHost(null);
          }}
        />
      )}
    </>
  );
}
