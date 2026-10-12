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
  /** This turn plus every later one the revert removes. */
  turnCount: number;
  reverting: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}

export function RevertTurnDialog({
  checkpoint,
  turnCount,
  reverting,
  onOpenChange,
  onConfirm,
}: RevertTurnDialogProps) {
  const later = Math.max(0, turnCount - 1);
  return (
    <AlertDialog
      open={checkpoint !== null}
      onOpenChange={(open) => {
        if (!reverting) onOpenChange(open);
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            {later === 0
              ? "Revert this turn?"
              : `Revert this turn and ${later} later ${later === 1 ? "turn" : "turns"}?`}
          </AlertDialogTitle>
          <AlertDialogDescription>
            Codemux puts the workspace files back to how they were before this
            turn, rewinds the agent&apos;s conversation, and removes{" "}
            {turnCount > 1 ? `these ${turnCount} turns` : "the turn"} from the
            transcript. Your prompt goes back into the composer. Right after
            reverting you can restore the current files; the conversation
            cannot be brought back.
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
            {reverting
              ? "Reverting…"
              : turnCount > 1
                ? `Revert ${turnCount} turns`
                : "Revert turn"}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
