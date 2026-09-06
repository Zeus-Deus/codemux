/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { AsyncQuestionPanel } from "./AsyncQuestionPanel";
import type { AsyncQuestionItem } from "@/lib/agent-chat/types";
import { agentChatAnswerQuestion } from "@/tauri/commands";
vi.mock("@/tauri/commands", () => ({ agentChatAnswerQuestion: vi.fn() }));
vi.mock("@/stores/agent-chat-store", () => ({
  useAgentChatStore: { getState: () => ({ applyEvent: vi.fn() }) },
}));
const item = (id = "q1"): AsyncQuestionItem => ({
  kind: "async_question",
  id,
  seq: 1,
  question: {
    id,
    target: "native",
    source_item_id: id,
    source_turn_id: "turn",
    text: "",
    questions: [{ title: "Which storage?", options: ["SQLite", "PostgreSQL"] }],
  },
  resolution: { status: "pending" },
});
afterEach(cleanup);
beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  vi.mocked(agentChatAnswerQuestion).mockResolvedValue({
    status: "answered",
    submission_id: "sent",
    answers: ["SQLite"],
    delivery: { kind: "inflight", turn_id: "turn" },
  });
});

describe("async question panel", () => {
  it("shows continuing work without submitting any suggested option", () => {
    render(<AsyncQuestionPanel threadId="thread" items={[item()]} working />);
    expect(screen.getByText("Work is continuing")).toBeInTheDocument();
    expect(agentChatAnswerQuestion).not.toHaveBeenCalled();
  });
  it("submits the real answer through the dedicated command", async () => {
    render(<AsyncQuestionPanel threadId="thread" items={[item()]} working />);
    fireEvent.click(screen.getByText("SQLite"));
    fireEvent.submit(screen.getByText("SQLite").closest("form")!);
    await waitFor(() =>
      expect(agentChatAnswerQuestion).toHaveBeenCalledWith(
        "thread",
        "q1",
        expect.objectContaining({
          action: "answer",
          answers: ["SQLite"],
          submission_id: expect.any(String),
        }),
      ),
    );
  });
  it("retains typed drafts across remounts and does not steal focus", () => {
    const input = document.createElement("textarea");
    document.body.append(input);
    input.focus();
    const view = render(
      <AsyncQuestionPanel threadId="thread" items={[item()]} working />,
    );
    expect(document.activeElement).toBe(input);
    fireEvent.change(screen.getByPlaceholderText("Something else…"), {
      target: { value: "Use an in-memory store" },
    });
    view.unmount();
    render(<AsyncQuestionPanel threadId="thread" items={[item()]} working />);
    expect(screen.getByPlaceholderText("Something else…")).toHaveValue(
      "Use an in-memory store",
    );
    input.remove();
  });
  it("supports free-text-only questions and leaves the normal composer keyboard alone", () => {
    const q = item();
    q.question.questions[0].options = [];
    render(<AsyncQuestionPanel threadId="thread" items={[q]} working />);
    expect(screen.getByPlaceholderText("Your answer…")).toBeInTheDocument();
    fireEvent.keyDown(document, { key: "Enter" });
    expect(agentChatAnswerQuestion).not.toHaveBeenCalled();
  });
  it("navigates multiple pending sets without overwriting them", () => {
    const second = item("q2");
    second.question.questions[0].title = "Which theme?";
    render(
      <AsyncQuestionPanel threadId="thread" items={[item(), second]} working />,
    );
    expect(screen.getByText("2 questions pending")).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("Next question set"));
    expect(screen.getByText("Which theme?")).toBeInTheDocument();
  });
  it("requires an explicit resend when delivery is unknown", () => {
    const q = item();
    q.resolution = {
      status: "unknown",
      submission_id: "old",
      answers: ["SQLite"],
      message: "Connection lost",
    };
    render(
      <AsyncQuestionPanel threadId="thread" items={[q]} working={false} />,
    );
    expect(
      screen.getByText("Sending again may duplicate your answer."),
    ).toBeInTheDocument();
    expect(agentChatAnswerQuestion).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText("Check delivery"));
    expect(agentChatAnswerQuestion).toHaveBeenCalledWith("thread", "q1", {
      action: "reconcile",
    });
  });
});
