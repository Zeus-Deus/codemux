import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  AssistantMessageItem,
  UserMessageItem,
} from "@/lib/agent-chat/types";

vi.mock("./ChatMarkdown", () => ({
  ChatMarkdown: ({ children }: { children: ReactNode }) => (
    <span data-rendered-markdown>{children}</span>
  ),
}));

import { AssistantMessage } from "./AssistantMessage";
import { UserMessage } from "./UserMessage";

function userItem(text: string): UserMessageItem {
  return { kind: "user_message", id: "user-1", seq: 1, text };
}

function assistantItem(
  text: string,
  streaming = false,
): AssistantMessageItem {
  return {
    kind: "assistant_message",
    id: "assistant-1",
    seq: 2,
    turn_id: "turn-1",
    text,
    streaming,
  };
}

let writeText: ReturnType<typeof vi.fn>;

beforeEach(() => {
  writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(window, "isSecureContext", {
    configurable: true,
    value: true,
  });
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText },
  });
});

afterEach(() => {
  // The suite runs without `globals`, so RTL's auto-cleanup is not registered.
  cleanup();
  vi.useRealTimers();
});

describe("message copy action", () => {
  it("copies the user's prompt verbatim", async () => {
    render(<UserMessage item={userItem("  line one\nline two  ")} />);

    fireEvent.click(screen.getByRole("button", { name: "Copy prompt" }));

    await vi.waitFor(() => {
      expect(writeText).toHaveBeenCalledWith("  line one\nline two  ");
      expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
    });
  });

  it("copies the assistant's Markdown source, not its rendered output", async () => {
    render(<AssistantMessage item={assistantItem("**bold** and `code`")} />);

    fireEvent.click(screen.getByRole("button", { name: "Copy response" }));

    await vi.waitFor(() => {
      expect(writeText).toHaveBeenCalledWith("**bold** and `code`");
    });
  });

  it("offers nothing to copy on an image-only prompt", () => {
    render(<UserMessage item={userItem("")} />);
    expect(screen.queryByTestId("message-copy-button")).toBeNull();
  });

  it("waits for the answer to settle before offering to copy it", () => {
    const { rerender } = render(
      <AssistantMessage item={assistantItem("half a th", true)} />,
    );
    expect(screen.queryByTestId("message-copy-button")).toBeNull();

    rerender(<AssistantMessage item={assistantItem("half a thought", false)} />);
    expect(screen.getByTestId("message-copy-button")).toBeInTheDocument();
  });

  it("reveals every footer action off the same rule, so they fade in together", () => {
    render(<UserMessage item={userItem("prompt")} onRevert={() => undefined} />);
    const copy = screen.getByTestId("message-copy-button");
    const revert = screen.getByTestId("revert-turn-button");

    // Sharing a parent means they also have to share a trigger: a Revert that
    // appeared on hover while Copy waited for focus would read as a glitch.
    expect(copy.parentElement).toBe(revert.parentElement);
    for (const reveal of [
      "opacity-0",
      "pointer-events-none",
      "group-hover:opacity-100",
      "group-focus-within:opacity-100",
      "pointer-coarse:opacity-100",
    ]) {
      expect(copy.className).toContain(reveal);
      expect(revert.className).toContain(reveal);
    }
  });

  it("ignores repeat clicks while the confirmation is up, then re-arms", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    render(<AssistantMessage item={assistantItem("once")} />);
    const button = screen.getByTestId("message-copy-button");

    fireEvent.click(button);
    await vi.waitFor(() =>
      expect(button).toHaveAttribute("aria-label", "Copied"),
    );
    fireEvent.click(button);
    expect(writeText).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(1500);
    await vi.waitFor(() =>
      expect(button).toHaveAttribute("aria-label", "Copy response"),
    );
    fireEvent.click(button);
    await vi.waitFor(() => expect(writeText).toHaveBeenCalledTimes(2));
  });

  it("falls back to execCommand when the async clipboard is unavailable", async () => {
    // The remote web client can be a plain-HTTP origin, where
    // `navigator.clipboard` is undefined.
    Object.defineProperty(window, "isSecureContext", {
      configurable: true,
      value: false,
    });
    const execCommand = vi.fn().mockReturnValue(true);
    Object.defineProperty(document, "execCommand", {
      configurable: true,
      value: execCommand,
    });

    render(<UserMessage item={userItem("over plain http")} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy prompt" }));

    await vi.waitFor(() => {
      expect(execCommand).toHaveBeenCalledWith("copy");
      expect(writeText).not.toHaveBeenCalled();
      expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
    });
  });
});
