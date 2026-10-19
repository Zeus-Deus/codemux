/**
 * The turn the backend sends on its own to a chat whose run the app stopped
 * mid-turn (quit or crash), once it opens again.
 */

/** Exact prefix the backend stamps on that turn. */
export const RESTART_RESUME_PREFIX = "[Resumed automatically after an app restart";

/** Whether a user turn is the backend's automatic resume after a restart. */
export function isRestartResumeText(text: string): boolean {
  return text.startsWith(RESTART_RESUME_PREFIX);
}
