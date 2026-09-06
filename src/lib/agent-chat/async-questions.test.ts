import { describe, expect, it } from "vitest";
import type { ProviderRuntimeEvent, UserQuestionSet } from "@/tauri/events";
import { applyEvent, createEmptyThreadState } from "./reducer";
import {
  replayParsed,
  lastTurnUnsettled,
  stampSearchSourceId,
} from "./hydrate";

const question: UserQuestionSet = {
  id: "question-1",
  target: "provider-1",
  source_item_id: "item-1",
  source_turn_id: "turn-1",
  text: "",
  questions: [{ title: "Storage?", options: ["SQLite", "PostgreSQL"] }],
};
const asked: ProviderRuntimeEvent = {
  type: "questions_asked",
  thread_id: "thread",
  question,
};
const answered: ProviderRuntimeEvent = {
  type: "question_resolved",
  thread_id: "thread",
  question_id: question.id,
  resolution: {
    status: "answered",
    submission_id: "reply-1",
    answers: ["SQLite"],
    delivery: { kind: "inflight", turn_id: "turn-1" },
  },
};
const running: ProviderRuntimeEvent = {
  type: "session_state_changed",
  thread_id: "thread",
  status: { status: "running", active_turn: "turn-1" },
};
const completed: ProviderRuntimeEvent = {
  type: "turn_completed",
  thread_id: "thread",
  turn_id: "turn-1",
  status: { kind: "success" },
  usage: null,
};
const fold = (events: ProviderRuntimeEvent[]) =>
  events.reduce((s, e) => applyEvent(s, e), createEmptyThreadState());

describe("async questions", () => {
  it("keeps later assistant output after the in-flight answer", () => {
    const state = fold([
      asked,
      {
        type: "content_delta",
        thread_id: "thread",
        turn_id: "turn-1",
        delta: { kind: "text", text: "Building the interface" },
      },
      answered,
      {
        type: "item_completed",
        thread_id: "thread",
        turn_id: "turn-1",
        item: { kind: "assistant_text", text: "Using SQLite" },
      },
    ]);
    expect(
      state.messages.map((item) =>
        item.kind === "assistant_message" || item.kind === "user_message"
          ? item.text
          : item.kind,
      ),
    ).toEqual([
      "async_question",
      "Building the interface",
      "Storage?\nSQLite",
      "Using SQLite",
    ]);
  });
  it("links searchable question and answer rows to durable event IDs", () => {
    const before = createEmptyThreadState();
    const withQuestion = stampSearchSourceId(
      before,
      applyEvent(before, asked),
      asked,
      10,
    );
    const withAnswer = stampSearchSourceId(
      withQuestion,
      applyEvent(withQuestion, answered),
      answered,
      12,
    );
    expect(withAnswer.messages[0]).toMatchObject({ source_event_id: 10 });
    expect(withAnswer.messages[1]).toMatchObject({ source_event_id: 12 });
  });
  it("keeps execution and approval state separate while questions arrive", () => {
    const state = fold([running, asked]);
    expect(state.streaming).toBe(true);
    expect(state.pendingRequestIds).toEqual([]);
  });
  it("does not manufacture a new turn for a mid-run answer", () => {
    const before = fold([running, asked]);
    const after = applyEvent(before, answered);
    expect(after.turnUnsettled).toBe(before.turnUnsettled);
    expect(after.streaming).toBe(true);
    expect(after.messages.find((m) => m.kind === "user_message")).toMatchObject(
      { inflight: true, in_reply_to: question.id, clientNonce: "reply-1" },
    );
  });
  it("deduplicates native events, answers and stale command results", () => {
    const state = fold([
      asked,
      asked,
      answered,
      answered,
      asked,
      {
        type: "question_resolved",
        thread_id: "thread",
        question_id: question.id,
        resolution: {
          status: "submitting",
          submission_id: "reply-1",
          answers: ["SQLite"],
        },
      },
    ]);
    expect(
      state.messages.filter((m) => m.kind === "async_question"),
    ).toHaveLength(1);
    expect(
      state.messages.filter((m) => m.kind === "user_message"),
    ).toHaveLength(1);
    expect(state.messages[0]).toMatchObject({
      resolution: { status: "answered" },
    });
  });
  it("keeps a question actionable after completion and cold replay", () => {
    const state = replayParsed([asked, completed]);
    expect(
      state.messages.find((m) => m.kind === "async_question"),
    ).toMatchObject({ resolution: { status: "pending" } });
    expect(state.pendingRequestIds).toEqual([]);
  });
  it("does not resurrect an answered question after cold replay", () => {
    const state = replayParsed([asked, answered, completed]);
    expect(
      state.messages.find((m) => m.kind === "async_question"),
    ).toMatchObject({ resolution: { status: "answered" } });
    expect(lastTurnUnsettled([asked, completed, answered])).toBe(false);
  });
  it("preserves source subagent identity", () => {
    const state = fold([
      {
        ...asked,
        question: { ...question, subagent_id: "child", target: "child" },
      },
    ]);
    expect(state.messages[0]).toMatchObject({
      question: { subagent_id: "child", target: "child" },
    });
  });
  it("records late answers as actual new turns, including fast completion before ack", () => {
    const late: ProviderRuntimeEvent = {
      ...answered,
      resolution: {
        status: "answered",
        submission_id: "late",
        answers: ["SQLite"],
        delivery: { kind: "new_turn", turn_id: "turn-2" },
      },
    };
    expect(lastTurnUnsettled([asked, completed, late])).toBe(true);
    const done: ProviderRuntimeEvent = { ...completed, turn_id: "turn-2" };
    expect(lastTurnUnsettled([asked, completed, done, late])).toBe(false);
    expect(fold([asked, completed, done, late]).turnUnsettled).toBe(false);
  });
});
