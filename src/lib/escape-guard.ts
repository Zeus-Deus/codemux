/**
 * Whether a page-level Escape handler should stand down.
 *
 * Full-screen destinations (Pull requests, Devices, …) close on Escape,
 * but so do the dialogs, menus and editors they contain. A page listener
 * that fires regardless closes the whole destination out from under an
 * open sheet — and takes any typed text with it. So the page declines the
 * key when something closer to the event already owns it.
 */
export function isEditableElement(el: Element | null | undefined): boolean {
  if (!el || !(el instanceof HTMLElement)) return false;
  const tag = el.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  return el.isContentEditable;
}

/**
 * Inside a Radix overlay — a dialog, sheet, popover, dropdown or the
 * wrapper any of them are portalled into.
 *
 * Deliberately not `[role="listbox"]`: a listbox is not by itself an
 * overlay, and a page's own focusable results list may be one.
 */
export function isInsideOverlay(el: Element | null | undefined): boolean {
  if (!el || typeof el.closest !== "function") return false;
  return (
    el.closest(
      '[role="dialog"], [role="alertdialog"], [role="menu"], [data-radix-popper-content-wrapper]',
    ) != null
  );
}

/** True when an Escape keydown belongs to something other than the page. */
export function escapeClaimedElsewhere(event: KeyboardEvent): boolean {
  // 1. Something closer to the event already claimed it.
  if (event.defaultPrevented) return true;

  const target = event.target as HTMLElement | null;
  const focused = document.activeElement as HTMLElement | null;

  // 2. The key belongs to whatever is being typed into.
  if (isEditableElement(target) || isEditableElement(focused)) return true;

  // 3. Focus is inside an overlay that dismisses itself on Escape. Radix
  //    does not `preventDefault` when it closes a dialog, so check 1 alone
  //    would not catch a sheet.
  if (isInsideOverlay(target) || isInsideOverlay(focused)) return true;

  // 4. A modal is open even though focus escaped it. The DOM has not been
  //    updated with the close Radix just scheduled, so an open dialog here
  //    means the key was for that dialog.
  return document.querySelector('[role="dialog"][data-state="open"]') != null;
}
