import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { ChatGptWelcome } from "./chatgpt-welcome";
import { useChatGptStore } from "@/stores/chatgpt-store";

beforeEach(() => {
  useChatGptStore.setState({
    busy: false, error: null, acknowledgeWelcome: vi.fn(), enterLocal: vi.fn(),
    status: {
      phase: "connected", attemptId: null, email: null, error: null, profiles: [],
      activeProfileId: null, welcomePending: true, installed: true,
    },
  });
});

afterEach(cleanup);

describe("ChatGptWelcome", () => {
  it("explains the plan and starts focus on the acknowledgement", async () => {
    render(<ChatGptWelcome enterWorkbench />);
    expect(await screen.findByRole("dialog")).toHaveTextContent(
      "Codex in Codemux now runs on your ChatGPT plan and its usage limits.",
    );
    expect(screen.getByRole("button", { name: "Got it" })).toHaveFocus();
  });
});
