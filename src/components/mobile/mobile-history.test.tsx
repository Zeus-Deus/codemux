/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  renderHook,
  screen,
  waitFor,
} from "@testing-library/react";
import type { LegendListRef } from "@legendapp/list/react";
import {
  registerMobileHistory,
  visibleMobileHistory,
  useMobileHistory,
} from "./mobile-history";
import { MobileHistorySheet } from "./mobile-history-sheet";
import { buildTranscriptSlots } from "@/components/chat/transcript-slots";

vi.mock("@/lib/toast", () => ({ toast: { error: vi.fn() } }));
const cleanups: (() => void)[] = [];
afterEach(() => {
  cleanup();
  cleanups.splice(0).forEach((fn) => fn());
});
function viewport() {
  const node = document.createElement("div");
  Object.defineProperties(node, {
    clientWidth: { value: 320 },
    clientHeight: { value: 400 },
  });
  document.body.append(node);
  cleanups.push(() => node.remove());
  return node;
}
function source(count = 60, workspaceId = "ws") {
  const node = viewport();
  const jump = vi.fn().mockResolvedValue(undefined);
  const value = {
    workspaceId,
    viewport: () => node,
    jump,
    entries: () =>
      Array.from({ length: count }, (_, i) => ({
        messageId: `m${i}`,
        slotIndex: i * 2,
        turnIndex: i,
        userText: `Prompt ${i + 1}`,
        replySnippet: `Reply ${i + 1}`,
      })),
  };
  cleanups.push(registerMobileHistory(value));
  return { value, node, jump };
}

describe("mobile conversation history", () => {
  it("uses only the visible conversation in the requested workspace", () => {
    const hidden = source();
    hidden.node.setAttribute("inert", "");
    const other = source(2, "other");
    const current = source();
    expect(visibleMobileHistory("ws")).toBe(current.value);
    expect(visibleMobileHistory("other")).toBe(other.value);
    expect(visibleMobileHistory("absent")).toBeUndefined();
  });
  it("pages newest-first prompts, searches all history and jumps by stable message id", async () => {
    const current = source();
    const onClose = vi.fn();
    render(
      <MobileHistorySheet
        workspaceId="ws"
        onClose={onClose}
        returnFocusRef={{ current: null }}
      />,
    );
    const rows = await screen.findAllByRole("button", { name: /Jump to turn/ });
    expect(rows).toHaveLength(50);
    expect(rows[0]).toHaveAccessibleName("Jump to turn 60: Prompt 60");
    fireEvent.click(screen.getByRole("button", { name: "Show older prompts" }));
    expect(
      screen.getAllByRole("button", { name: /Jump to turn/ }),
    ).toHaveLength(60);
    fireEvent.change(screen.getByRole("textbox", { name: "Find a prompt" }), {
      target: { value: "Prompt 2" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Jump to turn 2: Prompt 2" }),
    );
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
    expect(current.jump).toHaveBeenCalledWith("m1");
  });
  it("keeps the sheet open after a failed jump and explains empty history", async () => {
    const current = source(1);
    current.jump.mockRejectedValue(new Error("gone"));
    const onClose = vi.fn();
    const view = render(
      <MobileHistorySheet
        workspaceId="ws"
        onClose={onClose}
        returnFocusRef={{ current: null }}
      />,
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "Jump to turn 1: Prompt 1" }),
    );
    await waitFor(() => expect(current.jump).toHaveBeenCalled());
    expect(onClose).not.toHaveBeenCalled();
    view.rerender(
      <MobileHistorySheet
        workspaceId="empty"
        onClose={onClose}
        returnFocusRef={{ current: null }}
      />,
    );
    expect(
      await screen.findByText(
        "Open an agent conversation to view its history.",
      ),
    ).toBeInTheDocument();
  });
  it("cancels live following before jumping and unregisters when the transcript unmounts", async () => {
    const node = viewport();
    const events: string[] = [];
    const jump = vi.fn(async () => {
      events.push("jump");
    });
    const listRef = {
      current: {
        getScrollableNode: () => node,
        scrollToIndex: jump,
      } as unknown as LegendListRef,
    };
    const slots = buildTranscriptSlots([
      { kind: "user_message", id: "prompt-1", seq: 1, text: "Earlier prompt" },
    ]);
    const hook = renderHook(() =>
      useMobileHistory({
        enabled: true,
        workspaceId: "ws",
        threadKey: "thread",
        slots,
        listRef,
        onNavigate: () => {
          events.push("navigate");
        },
      }),
    );
    const current = visibleMobileHistory("ws")!;
    await act(async () => {
      await current.jump("prompt-1");
    });
    expect(events).toEqual(["navigate", "jump"]);
    expect(jump).toHaveBeenCalledWith({
      index: 0,
      animated: false,
      viewOffset: 10,
    });
    hook.unmount();
    expect(visibleMobileHistory("ws")).toBeUndefined();
  });
});
