/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

const { sendSideChat } = vi.hoisted(() => ({ sendSideChat: vi.fn() }));
vi.mock("@/lib/agent-chat/side-chat", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/agent-chat/side-chat")>()),
  sendSideChat,
}));
vi.mock("@/components/chat/ChatMarkdown", () => ({
  ChatMarkdown: ({ children }: { children: string }) => <div data-testid="markdown">{children}</div>,
}));

import type { SideChatState } from "@/lib/agent-chat/side-chat";
import { useProviderCapabilities } from "@/stores/provider-capabilities-store";
import type { ProviderChatCapabilities } from "@/tauri/types";

import { sideChatEffortName, sideChatModelName, SideChatPane } from "./side-chat-pane";

afterEach(() => {
  cleanup();
  sendSideChat.mockReset();
  useProviderCapabilities.setState({ claude: null, codex: null });
});

function withCaps(provider: "claude" | "codex", models: Array<{ id: string; label: string }>, labels = {}) {
  useProviderCapabilities.setState({
    [provider]: { models, effort_label_map: labels } as unknown as ProviderChatCapabilities,
  });
}

function chat(overrides: Partial<SideChatState> = {}): SideChatState {
  return {
    sideId: "side-1",
    threadId: "t-1",
    workspaceId: "ws-1",
    provider: "claude",
    model: null,
    effort: null,
    turns: [{ question: "What is the retry policy?", answer: "Six attempts.", status: "done" }],
    ended: false,
    ...overrides,
  };
}

describe("SideChatPane", () => {
  it("explains /btw when no side chat is open", () => {
    render(<SideChatPane chat={null} workspaceId="ws-1" cwd={null} />);
    expect(screen.getByText("No side chat open")).toBeInTheDocument();
    expect(screen.getByText(/to ask something on the side/)).toBeInTheDocument();
  });

  it("names the model and effort answering, with the provider logo", () => {
    withCaps("claude", [{ id: "claude-opus-5-5", label: "Claude Opus 5.5" }], { high: "High" });
    render(
      <SideChatPane
        chat={chat({ model: "claude-opus-5-5", effort: "high" })}
        workspaceId="ws-1"
        cwd={null}
      />,
    );
    const row = screen.getByTestId("side-chat-model");
    expect(row).toHaveTextContent("Answered by Opus 5.5·High effort");
    expect(row.querySelector("img")).toHaveAttribute("data-provider", "claude");
    // No picker: the row is plain text.
    expect(row.querySelector("button")).toBeNull();
    // It sits under the reply box, where the main composer has its scope
    // strip, so the two boxes line up across the panes.
    const box = screen.getByRole("textbox", { name: "Reply in this side chat" });
    expect(box.compareDocumentPosition(row) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(box.closest("[data-slot=input-group]")).toHaveClass("rounded-[22px]");
    // The send circle keeps the main composer's 5px inset: the primitive's
    // outward pull on an end button is cancelled, and the line height is a
    // whole pixel so the box stays exactly 44px.
    expect(screen.getByRole("button", { name: "Send reply" }).parentElement).toHaveClass(
      "has-[>button]:mr-0",
    );
    expect(box).toHaveClass("leading-[22px]");
  });

  it("falls back to the provider default model when the chat has no model id", () => {
    withCaps("codex", [{ id: "gpt-5.6-sol", label: "GPT-5.6 Sol" }]);
    render(<SideChatPane chat={chat({ provider: "codex" })} workspaceId="ws-1" cwd={null} />);
    expect(screen.getByTestId("side-chat-model")).toHaveTextContent("GPT-5.6 Sol");
  });

  it("renders the question as the transcript's user bubble", () => {
    render(<SideChatPane chat={chat()} workspaceId="ws-1" cwd={null} />);
    const bubble = screen.getByText("What is the retry policy?").parentElement!;
    expect(bubble.className).toContain("rounded-[14px_14px_5px_14px]");
    expect(screen.queryByTestId("revert-turn-button")).toBeNull();
  });

  it("shows the question and the rendered answer", () => {
    render(<SideChatPane chat={chat()} workspaceId="ws-1" cwd="/repo" />);
    expect(screen.getByText("What is the retry policy?")).toBeInTheDocument();
    expect(screen.getByTestId("markdown")).toHaveTextContent("Six attempts.");
  });

  it("shows progress while the first answer is pending", () => {
    render(
      <SideChatPane
        chat={chat({ turns: [{ question: "q", answer: "", status: "streaming" }] })}
        workspaceId="ws-1"
        cwd={null}
      />,
    );
    expect(screen.getByText("Thinking…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send reply" })).toBeDisabled();
  });

  it("sends a follow-up on Enter and clears the box", () => {
    render(<SideChatPane chat={chat()} workspaceId="ws-1" cwd={null} />);
    const box = screen.getByRole("textbox", { name: "Reply in this side chat" });
    fireEvent.change(box, { target: { value: "  and the backoff? " } });
    fireEvent.keyDown(box, { key: "Enter" });
    expect(sendSideChat).toHaveBeenCalledWith("side-1", "and the backoff?");
    expect(box).toHaveValue("");
  });

  it("Shift+Enter does not send", () => {
    render(<SideChatPane chat={chat()} workspaceId="ws-1" cwd={null} />);
    const box = screen.getByRole("textbox", { name: "Reply in this side chat" });
    fireEvent.change(box, { target: { value: "draft" } });
    fireEvent.keyDown(box, { key: "Enter", shiftKey: true });
    expect(sendSideChat).not.toHaveBeenCalled();
  });

  it("shows errors and disables the reply box once the fork ended", () => {
    render(
      <SideChatPane
        chat={chat({
          ended: true,
          turns: [{ question: "q", answer: "", status: "error", error: "Couldn't fork this chat." }],
        })}
        workspaceId="ws-1"
        cwd={null}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("Couldn't fork this chat.");
    expect(screen.getByRole("textbox", { name: "Reply in this side chat" })).toBeDisabled();
  });
});

describe("side chat labels", () => {
  it("drops the provider name the logo already shows", () => {
    expect(sideChatModelName("claude", "claude-opus-5-5", "Claude Opus 5.5")).toBe("Opus 5.5");
    expect(sideChatModelName("codex", "gpt-5.6-sol", "GPT-5.6 Sol")).toBe("GPT-5.6 Sol");
    expect(sideChatModelName("claude", "claude-opus-4-8", null)).toBe("Opus 4.8");
    expect(sideChatModelName("claude", null, null)).toBe("Default model");
  });

  it("uses the provider's effort labels", () => {
    expect(sideChatEffortName("xhigh", { xhigh: "Extra high" })).toBe("Extra high");
    expect(sideChatEffortName("max", {})).toBe("Max");
    expect(sideChatEffortName(null, {})).toBeNull();
  });
});
