import type { PaneNodeSnapshot } from "@/tauri/types";

export function findChatPane(
  root: PaneNodeSnapshot,
): PaneNodeSnapshot | null {
  if (root.kind === "agent_chat") return root;
  if (root.kind === "split") {
    for (const child of root.children) {
      const found = findChatPane(child);
      if (found) return found;
    }
  }
  return null;
}

/**
 * True if the tree contains at least one leaf pane (terminal,
 * browser, or agent_chat). A split whose children are all empty
 * returns false. `null` (no surface / no root) returns false.
 */
export function hasAnyPane(root: PaneNodeSnapshot | null): boolean {
  if (!root) return false;
  if (root.kind === "split") {
    return root.children.some(hasAnyPane);
  }
  return true;
}
