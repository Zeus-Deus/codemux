import { beforeEach, describe, expect, it } from "vitest";

import { useConversationSearchStore } from "./conversation-search-store";

describe("conversation search navigation store", () => {
  beforeEach(() => {
    useConversationSearchStore.setState({ target: null });
  });

  it("keeps a newer navigation when an older jump finishes late", () => {
    const first = useConversationSearchStore.getState().navigateTo({
      threadId: "thread-a",
      messageId: 1,
      turnId: null,
    });
    const second = useConversationSearchStore.getState().navigateTo({
      threadId: "thread-b",
      messageId: 2,
      turnId: "turn-b",
    });

    useConversationSearchStore.getState().clearHandled(first.nonce);
    expect(useConversationSearchStore.getState().target).toEqual(second);

    useConversationSearchStore.getState().clearHandled(second.nonce);
    expect(useConversationSearchStore.getState().target).toBeNull();
  });
});
