import { create } from "zustand";

/**
 * One-shot request to focus a draft's composer.
 *
 * "New agent" (sidebar button, Ctrl+N) often lands on the draft that is
 * already on screen, because the empty home draft is reused, so switching
 * drafts alone changes nothing the user can see. The request survives the
 * remount that happens when a different draft takes the slot, and the
 * surface showing `draftId` consumes it exactly once.
 */
interface DraftComposerFocusStore {
  request: { draftId: string; nonce: number } | null;
  requestFocus: (draftId: string) => void;
  clear: () => void;
}

let nonce = 0;

export const useDraftComposerFocusStore = create<DraftComposerFocusStore>(
  (set) => ({
    request: null,
    requestFocus: (draftId) => set({ request: { draftId, nonce: ++nonce } }),
    clear: () => set({ request: null }),
  }),
);
