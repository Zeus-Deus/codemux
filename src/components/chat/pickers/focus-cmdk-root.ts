/**
 * `PopoverContent.onOpenAutoFocus` handler that routes focus to the
 * right cmdk element when a popover opens.
 *
 * Why: cmdk's arrow-key / Enter / Escape handlers live on the Command
 * root element (a div with `tabIndex={-1}` and `[cmdk-root]` attribute).
 * Keydown events must originate on that element (or a descendant) for
 * cmdk to process them. Radix's default auto-focus lands on the first
 * tabbable element inside the popover — which is the `PopoverContent`
 * wrapper when nothing inside is focusable (arrow keys do nothing), or
 * whatever control happens to precede the search field in the DOM (a
 * filter rail, a tab strip — typing goes nowhere).
 *
 * So this handler intercepts Radix's auto-focus and picks the element
 * cmdk actually wants:
 *
 *  - the `CommandInput` (`[cmdk-input]`), when the popover has one — the
 *    search field must own focus so typing filters immediately, and it
 *    is a descendant of the root, so arrow keys still reach cmdk;
 *  - the `[cmdk-root]` div otherwise, which restores arrow-key
 *    navigation on input-less pickers.
 *
 * The input MUST win when present: neither shadcn's `CommandInput` nor
 * cmdk's `Command.Input` sets `autoFocus`, and cmdk's root keydown only
 * handles navigation keys — focusing the root on an input-bearing
 * popover leaves printable keystrokes with nowhere to go until the user
 * clicks the search box.
 */
export function focusCmdkOnOpen(event: Event): void {
  event.preventDefault();
  const content = event.currentTarget as HTMLElement | null;
  const target =
    content?.querySelector<HTMLElement>("[cmdk-input]") ??
    content?.querySelector<HTMLElement>("[cmdk-root]");
  target?.focus();
}
