/**
 * Mounted composers, for `host.composer`. Each Composer registers a small
 * handle while mounted; the "focused" one is whichever had focus last (or the
 * only one mounted).
 */
export interface ComposerHandle {
  workspaceId: string | null;
  threadId: string | null;
  getDraft(): string;
  insertText(text: string): void;
  focus(): void;
}

const composers = new Map<string, ComposerHandle>();
let lastFocused: string | null = null;

export function registerComposer(id: string, handle: ComposerHandle): () => void {
  composers.set(id, handle);
  return () => {
    if (composers.get(id) === handle) composers.delete(id);
    if (lastFocused === id) lastFocused = null;
  };
}

export function markComposerFocused(id: string): void {
  if (composers.has(id)) lastFocused = id;
}

/** The composer a plugin action targets: the last focused, else the only one. */
export function focusedComposer(): ComposerHandle | null {
  if (lastFocused && composers.has(lastFocused)) return composers.get(lastFocused)!;
  return composers.size === 1 ? [...composers.values()][0]! : null;
}

/** A new line unless the draft is empty
 *  or already ends with one. */
export function appendText(draft: string, text: string): string {
  return !text ? draft : draft + (draft && !draft.endsWith("\n") ? "\n" : "") + text;
}
