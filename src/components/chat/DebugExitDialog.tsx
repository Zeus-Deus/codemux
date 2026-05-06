import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";

// Overlay-manager note: the codemux-ui skill mentions a global
// overlay manager at `src/stores/overlay.ts`, but that file does
// not exist in this codebase yet. Other dialogs in
// `src/components/overlays/` (CloneDialog, NewWorkspaceDialog) use
// a per-feature `useUIStore` boolean as their open/close source of
// truth; this dialog uses `open` as a prop driven by the caller's
// Promise wrapper (`confirmDebugExit` in AgentChatPane) for the
// same effect. Radix's Dialog primitive handles the singleton
// modal-open state internally — Esc, overlay-click, and focus-
// trap all route through `onOpenChange(false)`, which the wrapper
// surfaces as `onChoose("cancel")`. Z-index layering inherits from
// `DialogContent`, which other dialogs share.

export type DebugExitChoice = "cleanup" | "leave" | "cancel";

interface Props {
  /** When false the dialog stays unmounted. The parent toggles this
   *  alongside the choice-resolving promise so the modal closes
   *  exactly once per decision. */
  open: boolean;
  /** Resolves the parent's `confirmDebugExit()` promise with the
   *  user's pick. The dialog calls this for all three buttons and on
   *  Esc / overlay-click (treated as "cancel"). */
  onChoose: (choice: DebugExitChoice) => void;
}

/** Confirm dialog rendered when the user removes the Debug pill while
 *  `// CODEMUX_DEBUG` markers exist in the workspace. Three exits:
 *  Remove markers (run cleanup turn), Leave them (drop pill, no
 *  cleanup), Cancel (keep pill on). */
export function DebugExitDialog({ open, onChoose }: Props) {
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) onChoose("cancel");
      }}
    >
      <DialogContent className="sm:max-w-[440px]">
        <DialogHeader>
          <DialogTitle>Remove debug markers?</DialogTitle>
          <DialogDescription>
            Codemux detected <code>CODEMUX_DEBUG</code> markers in your
            codebase. Clean them up before exiting Debug mode?
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" onClick={() => onChoose("cancel")}>
            Cancel
          </Button>
          <Button variant="outline" onClick={() => onChoose("leave")}>
            Leave them
          </Button>
          <Button
            onClick={() => onChoose("cleanup")}
            className="bg-foreground text-background hover:bg-foreground/90"
          >
            Remove markers
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
