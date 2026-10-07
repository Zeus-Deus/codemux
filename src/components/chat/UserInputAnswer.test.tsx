/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

import type { PermissionRequestItem } from "@/lib/agent-chat/types";
import type { ApprovalDecision } from "@/tauri/events";

import { USER_INPUT_SKIPPED_MESSAGE } from "./ComposerPendingInputPanel";
import { UserInputAnswer } from "./UserInputAnswer";

afterEach(() => cleanup());

function resolved(decision: ApprovalDecision): PermissionRequestItem {
  return {
    kind: "permission_request",
    id: "req-q",
    seq: 1,
    request_id: "req-q",
    turn_id: "turn-1",
    request_kind: "user-input",
    payload: { questions: [{ question: "Which?", header: "Pick" }] },
    tool_use_id: null,
    resolution: { state: "resolved", decision },
  };
}

describe("UserInputAnswer", () => {
  it("reads Skipped only when the user skipped the question", () => {
    render(
      <UserInputAnswer
        item={resolved({ decision: "deny", message: USER_INPUT_SKIPPED_MESSAGE })}
      />,
    );
    expect(screen.getByText("Skipped")).toBeInTheDocument();
  });

  it("does not call an interrupted question skipped or answered", () => {
    render(
      <UserInputAnswer
        item={resolved({ decision: "deny", message: "Tool request was aborted." })}
      />,
    );
    expect(screen.getByText("Not answered")).toBeInTheDocument();
  });

  it("echoes the chosen answer", () => {
    render(
      <UserInputAnswer
        item={resolved({
          decision: "allow",
          updated_input: { answers: { "Which?": "Option B" } },
        })}
      />,
    );
    expect(screen.getByText("Option B")).toBeInTheDocument();
  });
});
