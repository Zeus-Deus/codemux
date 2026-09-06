import { useEffect, useLayoutEffect, useRef } from "react";

/** Transient dialogs must not reopen or restore focus when retained transcript
 * effects reconnect. Ordinary rerenders keep the dialog and its owner intact. */
export function useDismissOnDisconnect(open: boolean, onOpenChange: (open: boolean) => void) {
  const committed = useRef({ open, onOpenChange });
  useLayoutEffect(() => {
    committed.current = { open, onOpenChange };
  }, [open, onOpenChange]);
  useEffect(() => () => {
    if (committed.current.open) committed.current.onOpenChange(false);
  }, []);
}
