import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import type { AgentChatCheckpointRecord } from "@/tauri/commands";

interface RestoreCheckpointDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The run-start checkpoint whose branch is shown in the copy. */
  checkpoint: AgentChatCheckpointRecord | null;
  /** True while the restore command is in flight — freezes the buttons. */
  restoring: boolean;
  onConfirm: () => void;
}

/**
 * Confirmation dialog for the run-start rollback checkpoint (issue #80),
 * shared by the per-pane {@link AgentChatPaneHeader} (split layouts) and the
 * GUI-chrome title-bar chat tab so the copy + confirm wiring live in ONE
 * place. Pair it with {@link useAgentChatCheckpointRestore}, which owns the
 * open/restoring state and the `handleRestoreConfirmed` mutation.
 */
export function RestoreCheckpointDialog({
  open,
  onOpenChange,
  checkpoint,
  restoring,
  onConfirm,
}: RestoreCheckpointDialogProps) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent
        // The pane header's pointer-down handler starts drag-to-swap; keep
        // dialog interactions out of it. Harmless where there's no such
        // handler (the title-bar tab portals the dialog to <body>).
        onPointerDown={(e) => e.stopPropagation()}
      >
        <AlertDialogHeader>
          <AlertDialogTitle>
            Restore workspace to before this run?
          </AlertDialogTitle>
          <AlertDialogDescription>
            Files go back to the snapshot taken when this chat session
            started{checkpoint?.branch ? ` (branch ${checkpoint.branch})` : ""}.
            Commits made during the run are undone, files the run created are
            deleted, and your pre-run changes come back as unstaged edits. A
            safety snapshot of the current state is kept under{" "}
            <code className="font-mono text-[11px]">
              refs/codemux/pre-restore
            </code>
            .
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={restoring}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            disabled={restoring}
            onClick={(e) => {
              // Keep the dialog open while the restore runs; the hook
              // closes it in its finally block.
              e.preventDefault();
              onConfirm();
            }}
            data-testid="restore-checkpoint-confirm"
          >
            {restoring ? "Restoring…" : "Restore"}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
