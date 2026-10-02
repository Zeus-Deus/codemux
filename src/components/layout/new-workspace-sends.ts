import {
  useChatDraftStore,
  type ChatDraft,
  type ChatDraftStore,
  type DraftId,
} from "@/stores/chat-draft-store";

/** When each draft's first send began, recorded only for sends that started
 *  in this app session.
 *
 *  `promoting` and `materializedTo` are persisted so an interrupted send can
 *  be recovered, which means a draft can rehydrate still claiming to be mid-
 *  send long after the process doing the sending died. Hydration is
 *  synchronous (localStorage) and completes inside `create()`, before this
 *  subscriber attaches, so a rehydrated draft is never seen *transitioning*
 *  into `promoting` and never earns a "creating" card. */
const startedAt = new Map<DraftId, number>();

useChatDraftStore.subscribe((state, prev) => {
  for (const draft of Object.values(state.draftsById)) {
    if (!draft.promoting) continue;
    if (prev.draftsById[draft.draftId]?.promoting) continue;
    startedAt.set(draft.draftId, Date.now());
  }
  for (const id of startedAt.keys()) {
    if (!(id in state.draftsById)) startedAt.delete(id);
  }
});

export function promotionStartedAt(draftId: DraftId): number | undefined {
  return startedAt.get(draftId);
}

/** First sends from this session that are creating a brand-new workspace:
 *  in flight, or promoted and still inside the draft store's post-promotion
 *  grace window. An existing-workspace draft creates nothing, so it never
 *  gets a stand-in. */
export function selectNewWorkspaceDraftsInFlight(
  s: ChatDraftStore,
): ChatDraft[] {
  const out: ChatDraft[] = [];
  for (const draft of Object.values(s.draftsById)) {
    if (draft.target.kind === "existing_workspace") continue;
    if (!startedAt.has(draft.draftId)) continue;
    if (draft.promoting || draft.promotedTo !== null) out.push(draft);
  }
  return out;
}
