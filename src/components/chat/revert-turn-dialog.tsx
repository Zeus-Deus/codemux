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
import type { AgentChatTurnCheckpointRecord } from "@/tauri/commands";

interface RevertTurnDialogProps {
  checkpoint: AgentChatTurnCheckpointRecord | null;
  reverting: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}

export function RevertTurnDialog({
  checkpoint,
  reverting,
  onOpenChange,
  onConfirm,
}: RevertTurnDialogProps) {
  return (
    <AlertDialog
      open={checkpoint !== null}
      onOpenChange={(open) => {
        if (!reverting) onOpenChange(open);
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Revert this turn and everything after it?</AlertDialogTitle>
          <AlertDialogDescription>
            Codemux will restore the workspace, rewind the Codex conversation,
            and remove this turn and later turns from the transcript. Your
            current workspace state is kept in a hidden Git safety snapshot.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={reverting}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            disabled={reverting}
            onClick={(event) => {
              event.preventDefault();
              onConfirm();
            }}
            data-testid="revert-turn-confirm"
          >
            {reverting ? "Reverting…" : "Revert turns"}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
