/**
 * `PopoverContent.onOpenAutoFocus` handler that puts focus on the
 * cmdk root div when a popover opens.
 *
 * Why: cmdk's arrow-key / Enter / Escape handlers live on the Command
 * root element (a div with `tabIndex={-1}` and `[cmdk-root]` attribute).
 * Keydown events must originate on that element (or a descendant) for
 * cmdk to process them. When we ship pickers without a `CommandInput`,
 * nothing in the popover is naturally focusable, so Radix's default
 * auto-focus lands on the `PopoverContent` wrapper — which sits
 * *outside* cmdk's keydown scope. Arrow keys do nothing.
 *
 * This handler intercepts Radix's auto-focus, finds the cmdk root
 * inside the popover content, and focuses it explicitly. Safe on
 * popovers that still have a `CommandInput` too — the input's own
 * `autoFocus` already handles that case, and we'd just be redundantly
 * focusing the root.
 */
export function focusCmdkRootOnOpen(event: Event): void {
  event.preventDefault();
  const content = event.currentTarget as HTMLElement | null;
  const root = content?.querySelector<HTMLElement>("[cmdk-root]");
  root?.focus();
}
