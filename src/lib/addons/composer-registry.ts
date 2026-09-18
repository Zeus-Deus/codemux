import { addonError } from "./types";
export interface ComposerTarget {
  workspaceId: string;
  append(text: string): number;
}
const composers = new Map<string, ComposerTarget>();
const listeners = new Set<() => void>();
export function subscribeAddonComposers(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
function changed() {
  for (const listener of listeners) listener();
}
export function registerAddonComposer(
  id: string,
  target: ComposerTarget,
): () => void {
  composers.set(id, target);
  changed();
  return () => {
    if (composers.get(id) === target) {
      composers.delete(id);
      changed();
    }
  };
}
export function composerForWorkspace(workspaceId: string): string | null {
  const matches = [...composers].filter(
    ([, target]) => target.workspaceId === workspaceId,
  );
  // Never guess between multiple mounted drafts. Actions in a composer pass
  // their own ID; a panel can only use an unambiguous current target.
  return matches.length === 1 ? matches[0][0] : null;
}
export function addonComposer(id: string, workspaceId: string): ComposerTarget {
  const target = composers.get(id);
  if (!target || target.workspaceId !== workspaceId)
    throw addonError(
      "NO_COMPOSER",
      "This chat composer is no longer available",
    );
  return target;
}
export function appendAddonText(draft: string, text: string): string {
  if (new TextEncoder().encode(text).byteLength > 32768)
    throw addonError("INVALID_MESSAGE", "Insertion exceeds 32 KiB");
  return !text
    ? draft
    : draft + (draft && !draft.endsWith("\n") ? "\n" : "") + text;
}
