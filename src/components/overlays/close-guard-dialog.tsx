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
import { useCloseGuardStore } from "@/lib/close-guard";

/** Asks before a tab or pane close would interrupt a working agent or a
 *  running process. Radix focuses Cancel, so Enter keeps the work alive. */
export function CloseGuardDialog() {
  const prompt = useCloseGuardStore((s) => s.prompt);

  return (
    <AlertDialog
      open={prompt !== null}
      onOpenChange={(open) => {
        if (!open) prompt?.cancel();
      }}
    >
      <AlertDialogContent size="sm" data-testid="close-guard-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle>Close “{prompt?.title}”?</AlertDialogTitle>
          <AlertDialogDescription>{prompt?.reason}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            onClick={() => prompt?.confirm()}
            data-testid="close-guard-confirm"
          >
            Close
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
