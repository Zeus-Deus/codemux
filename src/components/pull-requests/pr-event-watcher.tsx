import { usePrEventToasts } from "@/lib/pr-event-toasts";

/**
 * Renders nothing; watches the pull-request overview for the two events
 * that earn a toast, and publishes the index pull-request links resolve
 * against.
 *
 * A component rather than a hook call in `App` so that a poll every
 * thirty seconds re-renders this empty node instead of the whole
 * application tree. Mounted beside the `Toaster` — above every
 * full-screen destination, so it keeps watching while you are in
 * Settings or on the Pull Requests page.
 */
export function PrEventWatcher() {
  usePrEventToasts();
  return null;
}
