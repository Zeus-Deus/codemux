/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { LAZY_TOOL_RESULT_KEY } from "@/lib/agent-chat/lazy-tool-result";
import type { ToolCallItem } from "@/lib/agent-chat/types";
import { useAgentChatStore } from "@/stores/agent-chat-store";

import { ToolCallBody } from "./ToolCallBodies";

vi.mock("@/tauri/commands", () => ({
  agentChatGetToolResult: vi.fn(),
  agentChatGetToolResultImage: vi.fn(),
  readChatImage: vi.fn(),
}));

vi.mock("@/lib/clipboard", () => ({ copyToClipboard: vi.fn().mockResolvedValue(true) }));
import { copyToClipboard } from "@/lib/clipboard";
vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc: (path: string) => `asset:${path}` }));
import { agentChatGetToolResult, agentChatGetToolResultImage } from "@/tauri/commands";

afterEach(() => { cleanup(); vi.useRealTimers(); vi.unstubAllGlobals(); });

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

function imageStub(): ToolCallItem {
  const item = stubbedTool();
  return { ...item, result_content: { [LAZY_TOOL_RESULT_KEY]: {
    row_id: ROW_ID, bytes: 96 * 1024, preview: "evidence", line_count: 1, has_images: true,
    images: [{ pointer: "/1" }],
  } } };
}

function fullPayload(text: unknown): string {
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
    vi.mocked(agentChatGetToolResultImage).mockReset();
    vi.mocked(copyToClipboard).mockResolvedValue(true);
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

  it("loads only the image for a thumbnail/lightbox, and copies the full original on demand", async () => {
    vi.mocked(agentChatGetToolResultImage).mockResolvedValue({ src: "data:image/png;base64,AAAA", local: false });
    const content = [{ type: "text", text: "evidence" }, { type: "image", source: { type: "base64", data: "AAAA" } }];
    vi.mocked(agentChatGetToolResult).mockResolvedValue(fullPayload(content));
    const item = stubbedTool();
    const envelope = item.result_content as { [LAZY_TOOL_RESULT_KEY]: { has_images: boolean; images?: { pointer: string }[] } };
    envelope[LAZY_TOOL_RESULT_KEY].has_images = true;
    envelope[LAZY_TOOL_RESULT_KEY].images = [{ pointer: "/1" }];
    render(<ToolCallBody item={item} />);
    await waitFor(() => expect(screen.getByRole("button", { name: "Open image" })).toBeInTheDocument());
    expect(agentChatGetToolResultImage).toHaveBeenCalledWith(ROW_ID, "/1");
    expect(agentChatGetToolResult).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Open image" }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    await userEvent.click(screen.getByRole("button", { name: "Copy full result" }));
    await waitFor(() => expect(copyToClipboard).toHaveBeenCalledWith(JSON.stringify(content, null, 2)));
    expect(screen.getByText(/line 1/)).toBeInTheDocument();
  });

  it("shows inline compatibility sources without requesting an unreachable handle", () => {
    const item = stubbedTool({ result_content: { [LAZY_TOOL_RESULT_KEY]: {
      row_id: ROW_ID, bytes: 40_000, preview: "svg", line_count: 1, has_images: true,
      images: [{ pointer: "/0", src: "data:image/svg+xml;base64,AAAA" }],
    } } });
    render(<ToolCallBody item={item} />);
    expect(screen.getByRole("img")).toHaveAttribute("src", "data:image/svg+xml;base64,AAAA");
  });

  it("reuses a bounded binary cache across hosted remounts and revokes idle URLs", async () => {
    vi.mocked(agentChatGetToolResultImage)
      .mockResolvedValueOnce({ src: "/managed/tool.png", local: true })
      .mockResolvedValueOnce({ src: "data:image/png;base64,AQID", local: false });
    const fetchMock = vi.fn().mockResolvedValue({ blob: async () => new Blob([new Uint8Array([1, 2, 3])]) });
    vi.stubGlobal("fetch", fetchMock);
    Object.defineProperty(URL, "createObjectURL", { configurable: true, value: vi.fn(() => "blob:tool-image") });
    Object.defineProperty(URL, "revokeObjectURL", { configurable: true, value: vi.fn() });
    const item = imageStub();
    const view = render(<ToolCallBody item={item} />);
    await waitFor(() => expect(screen.getByRole("img")).toHaveAttribute("src", "asset:/managed/tool.png"));
    fireEvent.error(screen.getByRole("img"));
    await waitFor(() => expect(screen.getByRole("img")).toHaveAttribute("src", "blob:tool-image"));
    await userEvent.click(screen.getByRole("button", { name: "Open image" }));
    expect(screen.getByRole("dialog").querySelector("img")).toHaveAttribute("src", "blob:tool-image");
    expect(agentChatGetToolResultImage).toHaveBeenLastCalledWith(ROW_ID, "/1", true);
    expect(fetchMock).toHaveBeenCalledWith("data:image/png;base64,AQID");
    expect(agentChatGetToolResult).not.toHaveBeenCalled();
    view.unmount();
    expect(URL.revokeObjectURL).not.toHaveBeenCalled();
    const remount = render(<ToolCallBody item={item} />);
    expect(screen.getByRole("img")).toHaveAttribute("src", "blob:tool-image");
    expect(agentChatGetToolResultImage).toHaveBeenCalledTimes(2);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    vi.useFakeTimers();
    remount.unmount();
    vi.advanceTimersByTime(60_000);
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:tool-image");
  });

  it("evicts over-budget compressed bytes only after every mounted owner releases them", async () => {
    vi.mocked(agentChatGetToolResultImage).mockImplementation(async (rowId, _pointer, inline) =>
      inline ? { src: "data:image/png;base64,AQID", local: false } : { src: `/managed/${rowId}.png`, local: true });
    const blob = new Blob([new Uint8Array([1, 2, 3])]);
    // Two compressed 17 MiB images exceed the aggregate cache budget while
    // each remains below the backend's 25 MiB image ceiling.
    Object.defineProperty(blob, "size", { value: 17 * 1024 * 1024 });
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ blob: async () => blob }));
    let nextUrl = 0;
    Object.defineProperty(URL, "createObjectURL", { configurable: true, value: vi.fn(() => `blob:budget-${++nextUrl}`) });
    Object.defineProperty(URL, "revokeObjectURL", { configurable: true, value: vi.fn() });
    const item = imageStub();
    const first = render(<ToolCallBody item={item} />);
    const second = render(<ToolCallBody item={item} />);
    const third = render(<ToolCallBody item={imageStub()} />);
    await waitFor(() => expect(screen.getAllByRole("img")).toHaveLength(3));
    for (const image of screen.getAllByRole("img")) fireEvent.error(image);
    await waitFor(() => expect(screen.getAllByRole("img").every((image) => image.getAttribute("src")?.startsWith("blob:"))).toBe(true));
    expect(URL.revokeObjectURL).not.toHaveBeenCalled();
    first.unmount();
    expect(URL.revokeObjectURL).not.toHaveBeenCalled();
    second.unmount();
    expect(URL.revokeObjectURL).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("img")).toHaveAttribute("src", "blob:budget-2");
    vi.useFakeTimers();
    third.unmount();
    vi.advanceTimersByTime(60_000);
    expect(URL.revokeObjectURL).toHaveBeenCalledTimes(2);
  });

  it("bounds idle Blob entries and never evicts a mounted image", async () => {
    vi.mocked(agentChatGetToolResultImage).mockImplementation(async (rowId, _pointer, inline) =>
      inline ? { src: "data:image/png;base64,AQID", local: false } : { src: `/managed/${rowId}.png`, local: true });
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ blob: async () => new Blob(["image"]) }));
    let nextUrl = 0;
    Object.defineProperty(URL, "createObjectURL", { configurable: true, value: vi.fn(() => `blob:entry-${++nextUrl}`) });
    Object.defineProperty(URL, "revokeObjectURL", { configurable: true, value: vi.fn() });
    const items = Array.from({ length: 33 }, () => imageStub());
    const view = render(<>{items.map((item, index) => <ToolCallBody key={index} item={item} />)}</>);
    await waitFor(() => expect(screen.getAllByRole("img")).toHaveLength(33));
    for (const image of screen.getAllByRole("img")) fireEvent.error(image);
    await waitFor(() => expect(screen.getAllByRole("img").every((image) => image.getAttribute("src")?.startsWith("blob:"))).toBe(true));
    expect(URL.revokeObjectURL).not.toHaveBeenCalled();
    vi.useFakeTimers();
    view.unmount();
    expect(URL.revokeObjectURL).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(60_000);
    expect(URL.revokeObjectURL).toHaveBeenCalledTimes(33);
  });

  it("retries a failed image request without fetching the entire result", async () => {
    vi.mocked(agentChatGetToolResultImage).mockRejectedValueOnce(new Error("unavailable"))
      .mockResolvedValueOnce({ src: "data:image/png;base64,AAAA", local: false });
    render(<ToolCallBody item={imageStub()} />);
    const retry = await screen.findByRole("button", { name: "Image preview unavailable — retry" });
    await userEvent.click(retry);
    expect(await screen.findByRole("button", { name: "Open image" })).toBeInTheDocument();
    expect(agentChatGetToolResultImage).toHaveBeenCalledTimes(2);
    expect(agentChatGetToolResult).not.toHaveBeenCalled();
  });

  it("reports copy failure separately from full-result load", async () => {
    vi.mocked(agentChatGetToolResultImage).mockResolvedValue({ src: "data:image/png;base64,AAAA", local: false });
    vi.mocked(agentChatGetToolResult).mockResolvedValue(fullPayload("original"));
    vi.mocked(copyToClipboard).mockResolvedValue(false);
    render(<ToolCallBody item={imageStub()} />);
    await userEvent.click(screen.getByRole("button", { name: "Copy full result" }));
    expect(await screen.findByRole("button", { name: "Couldn't copy the full result — retry" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Load full output (96 KB)" })).toBeInTheDocument();
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
    serviceTier: "default",
    mode: "default" as const,
    modePriorPermissionMode: null,
    hasDebugActivity: false,
    debugActivityResolved: false,
    stagedAttachments: [],
    lastPersistedEventId: null,
  };
}
