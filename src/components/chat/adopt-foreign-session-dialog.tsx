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
import type { ForeignProjectAdoptPrompt } from "@/hooks/use-agent-chat-session-actions";

/**
 * Confirmation for a `/resume` pick that lives in a DIFFERENT project
 * (design R4). Adopting it runs the pane's thread in that project's
 * directory, so the jump is an explicit yes rather than a side effect
 * of picking a row. Same-checkout picks never reach this dialog.
 */
export function AdoptForeignSessionDialog({
  prompt,
  onOpenChange,
  onConfirm,
}: {
  prompt: ForeignProjectAdoptPrompt | null;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}) {
  return (
    <AlertDialog open={prompt !== null} onOpenChange={onOpenChange}>
      <AlertDialogContent data-testid="adopt-foreign-session-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle>Open this chat in another project?</AlertDialogTitle>
          <AlertDialogDescription>
            {prompt
              ? `"${prompt.session.title}" belongs to ${prompt.cwd}. Resuming it points this chat at that directory — the agent will read and write files there, not in the project you have open.`
              : ""}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel data-testid="adopt-foreign-session-cancel">
            Cancel
          </AlertDialogCancel>
          <AlertDialogAction
            onClick={(event) => {
              event.preventDefault();
              onConfirm();
            }}
            data-testid="adopt-foreign-session-confirm"
          >
            Open there
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
