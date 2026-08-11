import { create } from "zustand";

export interface ConversationSearchTarget {
  threadId: string;
  messageId: number | null;
  turnId: string | null;
  nonce: number;
}

interface ConversationSearchState {
  target: ConversationSearchTarget | null;
  navigateTo: (
    target: Omit<ConversationSearchTarget, "nonce">,
  ) => ConversationSearchTarget;
  clearHandled: (nonce: number) => void;
}

let nextNavigationNonce = 0;

/** One-shot bridge from a global search result to the chat virtualizer. The
 * target is installed before the backend activates/creates the pane, so a
 * freshly-mounted transcript cannot miss the navigation intent. */
export const useConversationSearchStore = create<ConversationSearchState>(
  (set) => ({
    target: null,
    navigateTo: (target) => {
      const next = { ...target, nonce: ++nextNavigationNonce };
      set({ target: next });
      return next;
    },
    clearHandled: (nonce) =>
      set((state) =>
        state.target?.nonce === nonce ? { target: null } : state,
      ),
  }),
);
