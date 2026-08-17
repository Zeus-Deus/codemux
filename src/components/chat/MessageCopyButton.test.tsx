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

const toastError = vi.hoisted(() => vi.fn());
vi.mock("@/lib/toast", () => ({ toast: { error: toastError } }));

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

function setSecureContext(value: boolean) {
  Object.defineProperty(window, "isSecureContext", {
    configurable: true,
    value,
  });
}

beforeEach(() => {
  toastError.mockClear();
  writeText = vi.fn().mockResolvedValue(undefined);
  setSecureContext(true);
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
      "group-hover/message:opacity-100",
      "group-focus-within/message:opacity-100",
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

  // The remote web client can be served from a plain-HTTP origin. Browsers
  // differ there: some drop `navigator.clipboard` entirely, others keep it and
  // reject the write. Every shape has to reach the execCommand fallback.
  it.each([
    [
      "the clipboard API is missing",
      () => {
        setSecureContext(false);
        Object.defineProperty(navigator, "clipboard", {
          configurable: true,
          value: undefined,
        });
      },
    ],
    [
      "the API exists but the origin is insecure",
      () => setSecureContext(false),
    ],
    [
      "the write rejects on a secure origin",
      () => writeText.mockRejectedValue(new Error("denied")),
    ],
  ])("falls back to execCommand when %s", async (_name, arrange) => {
    arrange();
    const execCommand = vi.fn().mockReturnValue(true);
    Object.defineProperty(document, "execCommand", {
      configurable: true,
      value: execCommand,
    });

    render(<UserMessage item={userItem("over plain http")} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy prompt" }));

    await vi.waitFor(() => {
      expect(execCommand).toHaveBeenCalledWith("copy");
      expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
    });
    expect(toastError).not.toHaveBeenCalled();
    // The fallback's scratch textarea must not outlive the copy.
    expect(document.querySelector("textarea")).toBeNull();
  });

  it("tells the user when every copy path is blocked", async () => {
    setSecureContext(false);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: undefined,
    });
    // A browser that refuses the fallback outright, the way a sandboxed or
    // permission-denied context does.
    Object.defineProperty(document, "execCommand", {
      configurable: true,
      value: vi.fn(() => {
        throw new Error("blocked");
      }),
    });

    render(<UserMessage item={userItem("nowhere to go")} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy prompt" }));

    await vi.waitFor(() =>
      expect(toastError).toHaveBeenCalledWith(expect.any(String)),
    );
    // No false confirmation, and no textarea left behind by the throw.
    expect(screen.queryByRole("button", { name: "Copied" })).toBeNull();
    expect(document.querySelector("textarea")).toBeNull();
  });
});

describe("message hover scope", () => {
  // An unnamed `group` here would also fire the unnamed `group-hover:` rules
  // inside a message's own content — inline images zoom-scale on hover — so
  // hovering anywhere on a message would animate every image in it.
  it("scopes the reveal to a named group on both message roots", () => {
    const { container: assistant } = render(
      <AssistantMessage item={assistantItem("answer")} />,
    );
    const { container: user } = render(<UserMessage item={userItem("ask")} />);

    for (const root of [assistant.firstElementChild, user.firstElementChild]) {
      expect(root).not.toBeNull();
      expect(root?.classList.contains("group/message")).toBe(true);
    }
    // The user root keeps its pre-existing unnamed group for the queued-turn
    // actions; the assistant root, which wraps rendered prose, must not.
    expect(assistant.firstElementChild?.classList.contains("group")).toBe(false);
  });
});
