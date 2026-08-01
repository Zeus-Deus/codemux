/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { LAZY_TOOL_RESULT_KEY } from "@/lib/agent-chat/lazy-tool-result";
import type { ToolCallItem } from "@/lib/agent-chat/types";
import { useAgentChatStore } from "@/stores/agent-chat-store";

import { ToolCallBody } from "./ToolCallBodies";

vi.mock("@/tauri/commands", () => ({
  agentChatGetToolResult: vi.fn(),
}));

import { agentChatGetToolResult } from "@/tauri/commands";

afterEach(() => cleanup());

const ROW_ID = 4242;

function stubbedTool(overrides: Partial<ToolCallItem> = {}): ToolCallItem {
  return {
    kind: "tool_call",
    id: "tool-1",
    seq: 0,
    tool_use_id: "tu-1",
    tool_name: "Bash",
    input: { command: "cat huge.log" },
    status: "done",
    result_content: {
      [LAZY_TOOL_RESULT_KEY]: {
        row_id: ROW_ID,
        bytes: 96 * 1024,
        preview: "line 1\nline 2\nline 3",
        line_count: 4000,
        has_images: false,
      },
    },
    approval_request_id: null,
    ...overrides,
  };
}

function fullPayload(text: string): string {
  return JSON.stringify({
    type: "item_completed",
    thread_id: "t",
    turn_id: "turn-1",
    item: { kind: "tool_result", tool_use_id: "tu-1", content: text, is_error: false },
  });
}

describe("lazy tool-result body", () => {
  beforeEach(() => {
    vi.mocked(agentChatGetToolResult).mockReset();
    useAgentChatStore.setState({ threads: {} });
  });

  it("renders the preview and the real line count while collapsed", () => {
    render(<ToolCallBody item={stubbedTool()} />);
    expect(screen.getByText(/line 1/)).toBeInTheDocument();
    // 4000 total minus the 3 preview lines, plus the transfer size so the
    // click is an informed one.
    expect(screen.getByRole("button")).toHaveTextContent("Show 3997 more lines");
    expect(screen.getByRole("button")).toHaveTextContent("96 KB");
    // Nothing is fetched until the user asks.
    expect(vi.mocked(agentChatGetToolResult)).not.toHaveBeenCalled();
  });

  it("fetches the body on demand and swaps it into the store item", async () => {
    vi.mocked(agentChatGetToolResult).mockResolvedValue(
      fullPayload("the whole thing"),
    );
    // A slice holding the stubbed item, so the resolve has somewhere to land.
    useAgentChatStore.setState({
      threads: {
        t: {
          ...useAgentChatStore.getState().threads.t,
          ...emptyish(),
          messages: [stubbedTool()],
        },
      },
    } as never);

    render(<ToolCallBody item={stubbedTool()} />);
    await userEvent.click(screen.getByRole("button"));

    await waitFor(() => {
      const item = useAgentChatStore.getState().threads.t
        .messages[0] as ToolCallItem;
      expect(item.result_content).toBe("the whole thing");
    });
    expect(vi.mocked(agentChatGetToolResult)).toHaveBeenCalledWith(ROW_ID);
  });

  it("offers a retry when the fetch fails", async () => {
    vi.mocked(agentChatGetToolResult).mockRejectedValue(new Error("gone"));
    render(<ToolCallBody item={stubbedTool()} />);
    await userEvent.click(screen.getByRole("button"));
    await waitFor(() => {
      expect(screen.getByRole("button")).toHaveTextContent("retry");
    });
    // The preview is still on screen — a failed fetch never blanks the card.
    expect(screen.getByText(/line 1/)).toBeInTheDocument();

    vi.mocked(agentChatGetToolResult).mockResolvedValue(fullPayload("recovered"));
    await userEvent.click(screen.getByRole("button"));
    await waitFor(() => {
      expect(vi.mocked(agentChatGetToolResult)).toHaveBeenCalledTimes(2);
    });
  });

  it("treats a row that is no longer a tool result as a failure", async () => {
    vi.mocked(agentChatGetToolResult).mockResolvedValue(
      JSON.stringify({ type: "turn_completed" }),
    );
    render(<ToolCallBody item={stubbedTool()} />);
    await userEvent.click(screen.getByRole("button"));
    await waitFor(() => {
      expect(screen.getByRole("button")).toHaveTextContent("retry");
    });
  });
});

/** Minimal slice shape for the store write above. */
function emptyish() {
  return {
    messages: [],
    nextSeq: 1,
    streaming: false,
    pendingRequestIds: [],
    interrupted: false,
    stalled: null,
    model: null,
    permissionMode: "bypassPermissions",
    sessionLaunchMode: null,
    inputDraft: "",
    activeTurnId: null,
    resumeCursor: null,
    effort: null,
    contextWindow: null,
    fastMode: false,
    mode: "default" as const,
    modePriorPermissionMode: null,
    hasDebugActivity: false,
    debugActivityResolved: false,
    stagedAttachments: [],
    checkpoint: null,
    lastPersistedEventId: null,
  };
}
