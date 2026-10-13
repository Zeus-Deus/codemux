import { afterEach, expect, it } from "vitest";
import { useChatGptStore } from "./chatgpt-store";
import { useChatDraftStore } from "./chat-draft-store";

afterEach(() => {
  useChatGptStore.setState({ status: null });
  useChatDraftStore.setState({ draftsById: {}, activeHomeDraftId: null, activeDraftId: null, projectDraftIdByPath: {} });
});
it("starts new drafts with Codex only after a verified ChatGPT connection", () => {
  useChatGptStore.setState({ status: { phase: "connected", attemptId: null, email: null,
    error: null, profiles: [], activeProfileId: "test-profile", welcomePending: false, installed: true } });
  expect(useChatDraftStore.getState().getOrCreateHomeDraft().provider).toBe("codex");
});
