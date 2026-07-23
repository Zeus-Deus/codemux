/**
 * Module-level holder for the sidebar inbox's jump targets: the ordered list
 * of visible active-card workspace ids, in the exact order the user sees them
 * (filter-scoped, non-settled). SidebarInbox writes this from an effect; the
 * window-level keyboard handler reads it so a "jump to workspace N" shortcut
 * can resolve the Nth card without the handler having to reach into React.
 *
 * Settled rows are never jump targets — only the `activeCards` list feeds here.
 */

/** The default modifier the jump shortcuts bind to (Ctrl+1..9 are the terminal
 *  tab switches, so digit jumps use Alt to avoid the conflict). */
export const DEFAULT_JUMP_MODIFIER = "Alt";

let jumpTargets: readonly string[] = [];

/** Replace the ordered jump-target ids (visible active cards, in view order). */
export function setJumpTargets(ids: readonly string[]): void {
  jumpTargets = ids;
}

/**
 * Resolve the workspace id for a 1-based jump slot (1 → first visible card).
 * Returns null when the slot is out of range or nothing is mounted.
 */
export function getJumpTarget(n: number): string | null {
  if (!Number.isInteger(n) || n < 1) return null;
  return jumpTargets[n - 1] ?? null;
}
