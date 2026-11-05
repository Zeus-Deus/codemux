import type React from "react";

/** Does this keydown activate the sidebar row/card *itself*?
 *
 *  Inbox cards, settled rows and snoozed rows are all `role="button"`
 *  containers that also host their own buttons (PR chip, Un-settle, Wake now).
 *  Those inner buttons stop click propagation, but a keyboard activation on a
 *  focused inner button still bubbles its `keydown` up to the container — so
 *  without a target check, Enter on the PR chip would open the PR *and*
 *  activate the workspace, and Space would be swallowed by the container's
 *  `preventDefault()` before the button's own default could fire. Only a
 *  keydown whose target is the container node is a container activation. */
export function isRowActivationKey(
  e: React.KeyboardEvent<HTMLElement>,
): boolean {
  if (e.key !== "Enter" && e.key !== " ") return false;
  return e.target === e.currentTarget;
}

/** Shift+F10 or the Menu key: the keyboard's "open the context menu". */
export function isContextMenuKey(e: React.KeyboardEvent<HTMLElement>): boolean {
  return e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey);
}

/** Opens a Radix context menu bound to `el` from the keyboard. Webviews don't
 *  all turn Shift+F10 into a `contextmenu` event, and the ones that do report
 *  0,0, so dispatch one ourselves anchored just inside the element's top-left
 *  corner. */
export function openContextMenuAt(el: HTMLElement): void {
  const r = el.getBoundingClientRect();
  el.dispatchEvent(
    new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
      clientX: r.left + 12,
      clientY: r.top + r.height / 2,
    }),
  );
}
