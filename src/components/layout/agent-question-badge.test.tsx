import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import {
  setQuestionAttention,
  useQuestionAttention,
} from "@/stores/async-question-attention-store";
import { AgentQuestionBadge } from "./agent-question-badge";

afterEach(() => {
  cleanup();
  useQuestionAttention.setState({ revision: -1, workspaces: {} });
});
it("shows durable question attention and ignores stale snapshots", () => {
  render(<AgentQuestionBadge workspaceId="workspace" />);
  act(() =>
    setQuestionAttention({ revision: 2, workspaces: { workspace: 1 } }),
  );
  expect(screen.getByLabelText("1 unanswered agent question")).toBeTruthy();
  act(() => setQuestionAttention({ revision: 1, workspaces: {} }));
  expect(screen.getByLabelText("1 unanswered agent question")).toBeTruthy();
  act(() => setQuestionAttention({ revision: 3, workspaces: {} }));
  expect(screen.queryByLabelText("1 unanswered agent question")).toBeNull();
});
