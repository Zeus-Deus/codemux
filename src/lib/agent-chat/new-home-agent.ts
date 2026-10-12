import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useDraftComposerFocusStore } from "@/stores/draft-composer-focus-store";

/**
 * The lazy-creation "New agent" gesture, shared by the sidebar button and
 * Ctrl+N: show the home-directory draft and put the caret in its composer.
 *
 * `lockedToHome: true` opts the draft out of `DraftChatSurface`'s mount-time
 * auto-seed and submit-time salvage, both of which would otherwise redirect
 * it to whatever project workspace is active in the sidebar. The button's
 * tooltip promises "New chat in home directory", so that is honoured
 * literally.
 *
 * The focus request is what makes a repeat press visible: the empty home
 * draft is reused, so without it the page would not change at all.
 */
export function startNewHomeAgent(): void {
  const store = useChatDraftStore.getState();
  const draft = store.getOrCreateHomeDraft({ lockedToHome: true });
  store.setActiveDraft(draft.draftId);
  useDraftComposerFocusStore.getState().requestFocus(draft.draftId);
}
