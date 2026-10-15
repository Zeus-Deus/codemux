import { describe, expect, it } from "vitest";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import type { ChatWorkflowLink } from "@/stores/workflow-ui-store";
import { latestTranscriptEventId, projectChatWorkflows } from "./chat-workflow-projection";

const user = (id: string, seq: number, eventId?: number): ChatViewItem => ({ kind: "user_message", id, seq, text: id, ...(eventId != null ? { source_event_id: eventId } : {}) } as ChatViewItem);
const link = (runId: string, anchorEventId: number | null, createdAt = 1): ChatWorkflowLink => ({ runId, workspaceId: "ws", anchorEventId, createdAt });
const order = (items: ChatViewItem[]) => items.map((item) => item.id);

describe("chat workflow projection", () => {
  it("finds the newest persisted event", () => {
    expect(latestTranscriptEventId([user("a", 1, 10), user("b", 2), user("c", 3, 12)])).toBe(12);
    expect(latestTranscriptEventId([user("a", 1)])).toBeNull();
  });

  it("places a card where it was launched, before events persisted later", () => {
    const messages = [user("a", 1, 10), user("b", 2), user("c", 3, 11), user("d", 4, 12)];
    expect(order(projectChatWorkflows(messages, [link("run", 11)], "thread"))).toEqual(["a", "b", "c", "workflow:run", "d"]);
  });

  it("puts a card launched after everything at the end, after rows without event ids", () => {
    const messages = [user("a", 1, 10), user("b", 2)];
    expect(order(projectChatWorkflows(messages, [link("run", 10)], "thread"))).toEqual(["a", "b", "workflow:run"]);
  });

  it("orders cards launched at the same point by launch time", () => {
    const messages = [user("a", 1, 10)];
    expect(order(projectChatWorkflows(messages, [link("late", 10, 5), link("early", 10, 2)], "thread"))).toEqual(["a", "workflow:early", "workflow:late"]);
  });

  it("puts a card from an empty chat before the first persisted event", () => {
    expect(order(projectChatWorkflows([user("a", 1, 10)], [link("run", null)], "thread"))).toEqual(["workflow:run", "a"]);
    expect(order(projectChatWorkflows([], [link("run", null)], "thread"))).toEqual(["workflow:run"]);
  });

  it("returns the transcript untouched without a thread or links", () => {
    const messages = [user("a", 1, 10)];
    expect(projectChatWorkflows(messages, [], "thread")).toBe(messages);
    expect(projectChatWorkflows(messages, [link("run", 10)], null)).toBe(messages);
  });
});
