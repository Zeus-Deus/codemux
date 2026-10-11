import { describe, expect, it } from "vitest";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import { taskFixture } from "@/lib/delegation.test-fixtures";
import { projectRemoteTasks } from "./remote-task-projection";

const user = (seq: number, event: number): ChatViewItem => ({ kind: "user_message", id: `user-${seq}`, seq, text: "Task", source_event_id: event });
const assistant = (seq: number, event: number): ChatViewItem => ({ kind: "assistant_message", id: `assistant-${seq}`, seq, turn_id: "turn-1", text: "Answer", streaming: false, source_event_id: event });

describe("remote task receipt projection", () => {
  it("maps the native persisted event id to view sequence, not the other way around", () => {
    const messages = [user(0, 100), assistant(1, 101), user(2, 105)];
    const task = taskFixture({ parent_event_id: 101 });
    const projected = projectRemoteTasks(messages, [task]);
    expect(projected.map(item => item.id)).toEqual(["user-0", "assistant-1", "remote:task-1", "user-2"]);
    expect(projected[2].seq).toBeGreaterThan(1);
    expect(projected[2].seq).toBeLessThan(2);
    expect(projected[0]).toBe(messages[0]);
    expect(projected[1]).toBe(messages[1]);
    expect(messages).toHaveLength(3);
  });
  it("does not move a receipt when later parent turns arrive or status changes", () => {
    const task = taskFixture({ parent_event_id: 100 });
    const first = projectRemoteTasks([user(0, 100)], [task]).find(item => item.kind === "remote_task")!;
    const second = projectRemoteTasks([user(0, 100), assistant(1, 101)], [{ ...task, status: "completed" }]).find(item => item.kind === "remote_task")!;
    expect(second.seq).toBe(first.seq);
  });
  it("keeps receipt ordering stable and disjoint from the provider item ids", () => {
    const tasks = [taskFixture({ id: "b", parent_event_id: 100 }), taskFixture({ id: "a", parent_event_id: 100 })];
    expect(projectRemoteTasks([user(0, 100)], tasks).map(item => item.id)).toEqual(["user-0", "remote:a", "remote:b"]);
    expect(projectRemoteTasks([user(0, 100)], [...tasks].reverse()).map(item => item.id)).toEqual(["user-0", "remote:a", "remote:b"]);
  });
  it("preserves the exact provider array on the no-task fast path", () => {
    const messages = [user(0, 100)];
    expect(projectRemoteTasks(messages, [])).toBe(messages);
  });
});
