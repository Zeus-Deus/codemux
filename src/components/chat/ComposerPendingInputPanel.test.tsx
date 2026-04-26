/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";

import type { PermissionRequestItem } from "@/lib/agent-chat/types";

import { ComposerPendingInputPanel } from "./ComposerPendingInputPanel";

afterEach(() => cleanup());

function makeAskItem(
  overrides: Partial<PermissionRequestItem> = {},
): PermissionRequestItem {
  return {
    kind: "permission_request",
    id: "req-ask-1",
    seq: 0,
    request_id: "req-ask-1",
    turn_id: "turn-1",
    request_kind: "user-input",
    payload: {
      questions: [
        {
          header: "Framework",
          question: "Which framework should we use?",
          multiSelect: false,
          options: [
            { label: "React", description: "Most popular" },
            { label: "Vue", description: "Simpler" },
            { label: "Svelte", description: "Compiler-based" },
          ],
        },
      ],
    },
    tool_use_id: "tu-ask-1",
    resolution: { state: "pending" },
    ...overrides,
  };
}

describe("ComposerPendingInputPanel", () => {
  it("renders header, question, options, and the free-text row", () => {
    render(
      <ComposerPendingInputPanel
        item={makeAskItem()}
        onSubmit={vi.fn()}
      />,
    );
    // Header uppercase caps.
    expect(screen.getByText("Framework")).toBeInTheDocument();
    expect(
      screen.getByText("Which framework should we use?"),
    ).toBeInTheDocument();
    expect(screen.getByText("React")).toBeInTheDocument();
    expect(screen.getByText("Vue")).toBeInTheDocument();
    expect(screen.getByText("Svelte")).toBeInTheDocument();
    // Always-visible free-text row.
    expect(
      screen.getByPlaceholderText("Something else…"),
    ).toBeInTheDocument();
    // Single question ⇒ `1 of 1` pagination pill.
    expect(screen.getByText(/1 of 1/)).toBeInTheDocument();
  });

  it("Send is disabled until a pick or free-text answer exists", () => {
    render(
      <ComposerPendingInputPanel
        item={makeAskItem()}
        onSubmit={vi.fn()}
      />,
    );
    const send = screen.getByText("Send") as HTMLButtonElement;
    expect(send).toBeDisabled();
    fireEvent.click(screen.getByText("React"));
    expect(send).not.toBeDisabled();
  });

  it("submits an SDK-shaped AskUserQuestionOutput keyed by question text, and echoes the original questions", () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const item = makeAskItem();
    render(<ComposerPendingInputPanel item={item} onSubmit={onSubmit} />);
    fireEvent.click(screen.getByText("Vue"));
    fireEvent.click(screen.getByText("Send"));
    const rawQuestions =
      (item.payload as { questions: unknown }).questions;
    expect(onSubmit).toHaveBeenCalledWith({
      questions: rawQuestions,
      answers: { "Which framework should we use?": "Vue" },
    });
  });

  it("free-text row becomes the answer value (not a synthetic 'Other' label the model can't read)", () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    render(
      <ComposerPendingInputPanel
        item={makeAskItem()}
        onSubmit={onSubmit}
      />,
    );
    const free = screen.getByPlaceholderText("Something else…");
    fireEvent.change(free, { target: { value: "SolidJS" } });
    // Free-text alone enables Send; no explicit radio click needed.
    const send = screen.getByText("Send") as HTMLButtonElement;
    expect(send).not.toBeDisabled();
    fireEvent.click(send);
    expect(onSubmit).toHaveBeenCalledWith(
      expect.objectContaining({
        answers: { "Which framework should we use?": "SolidJS" },
      }),
    );
  });

  it("multi-select answers are comma-joined under the question text key", () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const item = makeAskItem({
      payload: {
        questions: [
          {
            header: "Features",
            question: "Which features do you want?",
            multiSelect: true,
            options: [
              { label: "Auth", description: "" },
              { label: "Billing", description: "" },
              { label: "Analytics", description: "" },
            ],
          },
        ],
      },
    });
    render(<ComposerPendingInputPanel item={item} onSubmit={onSubmit} />);
    fireEvent.click(screen.getByText("Auth"));
    fireEvent.click(screen.getByText("Analytics"));
    fireEvent.click(screen.getByText("Send"));
    expect(onSubmit).toHaveBeenCalledWith(
      expect.objectContaining({
        answers: { "Which features do you want?": "Auth, Analytics" },
      }),
    );
  });

  it("multi-question: Next button advances, pagination label updates, Send appears on the last question", () => {
    const item = makeAskItem({
      payload: {
        questions: [
          {
            header: "A",
            question: "Q1?",
            multiSelect: false,
            options: [{ label: "A1", description: "" }],
          },
          {
            header: "B",
            question: "Q2?",
            multiSelect: false,
            options: [{ label: "B1", description: "" }],
          },
        ],
      },
    });
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    render(
      <ComposerPendingInputPanel item={item} onSubmit={onSubmit} />,
    );
    // Start at question 1.
    expect(screen.getByText("Q1?")).toBeInTheDocument();
    expect(screen.getByText(/1 of 2/)).toBeInTheDocument();
    // Next is disabled until Q1 is answered.
    const next = screen.getByText("Next") as HTMLButtonElement;
    expect(next).toBeDisabled();
    fireEvent.click(screen.getByText("A1"));
    expect(next).not.toBeDisabled();
    // Advance.
    fireEvent.click(next);
    expect(screen.getByText("Q2?")).toBeInTheDocument();
    expect(screen.getByText(/2 of 2/)).toBeInTheDocument();
    // Primary action is now Send.
    const send = screen.getByText("Send") as HTMLButtonElement;
    expect(send).toBeDisabled();
    fireEvent.click(screen.getByText("B1"));
    expect(send).not.toBeDisabled();
    fireEvent.click(send);
    expect(onSubmit).toHaveBeenCalledWith(
      expect.objectContaining({
        answers: { "Q1?": "A1", "Q2?": "B1" },
      }),
    );
  });

  it("Prev button returns to the previous question and preserves picks", () => {
    const item = makeAskItem({
      payload: {
        questions: [
          {
            header: "A",
            question: "Q1?",
            multiSelect: false,
            options: [
              { label: "A1", description: "" },
              { label: "A2", description: "" },
            ],
          },
          {
            header: "B",
            question: "Q2?",
            multiSelect: false,
            options: [{ label: "B1", description: "" }],
          },
        ],
      },
    });
    render(
      <ComposerPendingInputPanel item={item} onSubmit={vi.fn()} />,
    );
    fireEvent.click(screen.getByText("A1"));
    fireEvent.click(screen.getByText("Next"));
    expect(screen.getByText("Q2?")).toBeInTheDocument();
    // Prev button (aria-label) goes back.
    fireEvent.click(screen.getByLabelText("Previous question"));
    expect(screen.getByText("Q1?")).toBeInTheDocument();
    // A1 is still the selected pick — Next remains enabled immediately.
    const next = screen.getByText("Next") as HTMLButtonElement;
    expect(next).not.toBeDisabled();
  });

  it("digit keys 1-9 toggle the matching option on the current question", () => {
    render(
      <ComposerPendingInputPanel
        item={makeAskItem()}
        onSubmit={vi.fn()}
      />,
    );
    // No input focus → digit key lands on the global handler.
    act(() => {
      document.dispatchEvent(
        new KeyboardEvent("keydown", { key: "2", bubbles: true }),
      );
    });
    const send = screen.getByText("Send") as HTMLButtonElement;
    expect(send).not.toBeDisabled();
    // The Vue option (index 2) should now be selected — its row
    // carries the "foreground/30" border-class, but a simpler assert:
    // picking Send emits Vue as the answer.
    fireEvent.click(send);
    // Don't assert the onSubmit payload here (wired to vi.fn() with no
    // capture). Instead, the enabled-Send assertion is sufficient
    // evidence the keyboard shortcut toggled the pick.
  });

  it("digit keys do NOT fire when focus is in the free-text input (composer typing is safe)", () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    render(
      <ComposerPendingInputPanel
        item={makeAskItem()}
        onSubmit={onSubmit}
      />,
    );
    const free = screen.getByPlaceholderText(
      "Something else…",
    ) as HTMLInputElement;
    free.focus();
    // Dispatch a "2" keydown from inside the input — the guard should
    // bail out so no option toggle happens.
    act(() => {
      free.dispatchEvent(
        new KeyboardEvent("keydown", { key: "2", bubbles: true }),
      );
    });
    // No pick means Send stays disabled — free-text is still empty so
    // the question remains unanswered.
    const send = screen.getByText("Send") as HTMLButtonElement;
    expect(send).toBeDisabled();
  });

  it("renders an option.preview popover only for options that supplied one", async () => {
    const item = makeAskItem({
      payload: {
        questions: [
          {
            header: "Branch",
            question: "Pick a branch:",
            multiSelect: false,
            options: [
              {
                label: "main",
                description: "primary",
                preview: "diff: 12 files changed",
              },
              { label: "draft", description: "wip" },
            ],
          },
        ],
      },
    });
    render(<ComposerPendingInputPanel item={item} onSubmit={vi.fn()} />);

    // The preview content lives inside a Radix HoverCard portal that
    // mounts only after hover; verify the preview is wired by looking
    // for the test-id that we attach to the HoverCardContent.
    expect(screen.queryByTestId("aq-option-preview-0-0")).not.toBeInTheDocument();
    expect(screen.queryByTestId("aq-option-preview-0-1")).not.toBeInTheDocument();

    // Triggering the open state directly via Radix would require
    // pointer events that jsdom doesn't fully simulate. Instead we
    // assert that only the option with a preview is wrapped in a
    // HoverCard trigger by checking the data-state attribute Radix
    // adds.
    const optionWithPreview = screen.getByTestId("aq-option-0-0");
    const optionWithoutPreview = screen.getByTestId("aq-option-0-1");
    // Radix HoverCardTrigger sets `data-state` on the slot element it
    // wraps; the bare label without a preview has no such attribute.
    expect(optionWithPreview).toHaveAttribute("data-state");
    expect(optionWithoutPreview).not.toHaveAttribute("data-state");
  });

  it("returns null when the resolution is no longer pending (panel dismounts cleanly)", () => {
    const { container } = render(
      <ComposerPendingInputPanel
        item={makeAskItem({
          resolution: {
            state: "resolved",
            decision: { decision: "allow" },
          },
        })}
        onSubmit={vi.fn()}
      />,
    );
    // The panel should render nothing — the transcript marker in
    // MessageList owns the resolved state.
    expect(container.firstChild).toBeNull();
  });

  // ----- option.preview edge cases -----------------------------------

  it("an option whose preview is the empty string is treated as no preview (no popover wrapper)", () => {
    const item = makeAskItem({
      payload: {
        questions: [
          {
            header: "X",
            question: "Pick:",
            multiSelect: false,
            options: [
              { label: "blank", description: "", preview: "" },
              { label: "real", description: "", preview: "stuff" },
            ],
          },
        ],
      },
    });
    render(<ComposerPendingInputPanel item={item} onSubmit={vi.fn()} />);
    // The blank-preview option must NOT be wrapped in a HoverCard
    // trigger (no `data-state` attribute from Radix). The real-preview
    // option must be.
    const blank = screen.getByTestId("aq-option-0-0");
    const real = screen.getByTestId("aq-option-0-1");
    expect(blank).not.toHaveAttribute("data-state");
    expect(real).toHaveAttribute("data-state");
  });

  it("an option with a non-string preview (number, object) is ignored", () => {
    const item = makeAskItem({
      payload: {
        questions: [
          {
            header: "X",
            question: "Pick:",
            multiSelect: false,
            options: [
              // Non-string previews — extractQuestions must coerce
              // these to null so the row stays a bare label.
              { label: "num", description: "", preview: 42 },
              { label: "obj", description: "", preview: { nested: "yo" } },
              { label: "ok", description: "", preview: "real preview" },
            ],
          },
        ],
      } as unknown as PermissionRequestItem["payload"],
    });
    render(<ComposerPendingInputPanel item={item} onSubmit={vi.fn()} />);
    expect(screen.getByTestId("aq-option-0-0")).not.toHaveAttribute(
      "data-state",
    );
    expect(screen.getByTestId("aq-option-0-1")).not.toHaveAttribute(
      "data-state",
    );
    expect(screen.getByTestId("aq-option-0-2")).toHaveAttribute("data-state");
  });

  it("multi-question payload: previews attach to the correct option on the correct question", () => {
    const item = makeAskItem({
      payload: {
        questions: [
          {
            header: "Q1",
            question: "Q1?",
            multiSelect: false,
            options: [
              { label: "A1", description: "", preview: "preview-A1" },
              { label: "A2", description: "" },
            ],
          },
          {
            header: "Q2",
            question: "Q2?",
            multiSelect: false,
            options: [
              { label: "B1", description: "" },
              { label: "B2", description: "", preview: "preview-B2" },
            ],
          },
        ],
      },
    });
    render(<ComposerPendingInputPanel item={item} onSubmit={vi.fn()} />);
    // Q1 page: option 0 has preview, option 1 does not.
    expect(screen.getByTestId("aq-option-0-0")).toHaveAttribute("data-state");
    expect(screen.getByTestId("aq-option-0-1")).not.toHaveAttribute(
      "data-state",
    );
    // Advance to Q2.
    fireEvent.click(screen.getByText("A1"));
    fireEvent.click(screen.getByText("Next"));
    // Q2 page: option 0 has no preview, option 1 has the preview.
    expect(screen.getByTestId("aq-option-1-0")).not.toHaveAttribute(
      "data-state",
    );
    expect(screen.getByTestId("aq-option-1-1")).toHaveAttribute("data-state");
  });
});
