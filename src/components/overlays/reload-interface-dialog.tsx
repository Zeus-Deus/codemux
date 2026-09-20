/**
 * Confirmation for the recovery reload (Ctrl+Shift+R, or the palette's
 * "Reload interface"). Mounted app-wide: it also carries the listener for the
 * app process's reload request, which is how the chord arrives on Linux.
 *
 * See `src/lib/interface-reload.ts` for the flow.
 */
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
import {
  cancelInterfaceReload,
  reloadInterfaceNow,
  useInterfaceReloadRequests,
  useInterfaceReloadStore,
} from "@/lib/interface-reload";

export function ReloadInterfaceDialog() {
  useInterfaceReloadRequests();
  const prompting = useInterfaceReloadStore((s) => s.prompting);

  return (
    <AlertDialog
      open={prompting}
      onOpenChange={(open) => {
        if (!open) cancelInterfaceReload();
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Reload the interface?</AlertDialogTitle>
          <AlertDialogDescription>
            The window redraws from scratch. Agents, terminals and anything
            running in them keep going — panes re-attach once the interface is
            back. Text you have typed but not sent in an open chat is lost.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            onClick={(event) => {
              event.preventDefault();
              reloadInterfaceNow();
            }}
            data-testid="reload-interface-confirm"
          >
            Reload interface
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
