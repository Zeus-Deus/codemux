import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, renderHook, screen } from "@testing-library/react";

import { TooltipProvider } from "@/components/ui/tooltip";
import type { UserMessageItem } from "@/lib/agent-chat/types";
import { useAgentChatStore } from "@/stores/agent-chat-store";

const commands = vi.hoisted(() => ({
  agentChatMoveQueuedTurn: vi.fn(),
  agentChatEditQueuedTurn: vi.fn(),
  agentChatSendSavedQueuedTurn: vi.fn(),
  agentChatCancelSavedQueuedTurn: vi.fn(),
  agentChatListQueuedTurns: vi.fn(),
}));
vi.mock("@/tauri/commands", () => commands);

import {
  QueuedTurnsList,
  useQueuedTurnEditor,
  type QueuedTurnEditor,
  type QueuedTurnsListProps,
} from "./queued-turn-controls";

afterEach(cleanup);
beforeEach(() => {
  for (const fn of Object.values(commands)) fn.mockReset().mockResolvedValue(true);
});

function item(queuedId: string, saved = false): UserMessageItem {
  return {
    kind: "user_message",
    id: queuedId,
    seq: 0,
    text: `text ${queuedId}`,
    clientNonce: `nonce-${queuedId}`,
    queued: { queuedId, ...(saved ? { saved } : {}) },
  };
}

function editor(overrides: Partial<QueuedTurnEditor> = {}): QueuedTurnEditor {
  return {
    editingId: null,
    editingHasImages: false,
    begin: vi.fn(),
    cancel: vi.fn(),
    save: vi.fn(async () => {}),
    ...overrides,
  };
}

function list(overrides: Partial<QueuedTurnsListProps> = {}) {
  const props: QueuedTurnsListProps = {
    threadId: "t",
    provider: "claude",
    queued: [item("a"), item("b"), item("c")],
    editor: editor(),
    streaming: true,
    canSteer: true,
    onSteer: vi.fn(),
    onInterrupt: vi.fn(),
    onRemove: vi.fn(),
    requestSendAnchor: vi.fn(),
    clearSendAnchor: vi.fn(),
    ...overrides,
  };
  render(
    <TooltipProvider>
      <QueuedTurnsList {...props} />
    </TooltipProvider>,
  );
  return props;
}

const rows = () => screen.queryAllByTestId("queued-turn-row");

describe("QueuedTurnsList", () => {
  it("shows an agent sender in the preview tooltip", async () => {
    list({queued:[{...item("agent"),created_by:"agent",sender_title:"Review agent",message_id:"message"}]});
    const preview=screen.getByText("text agent");
    fireEvent.focus(preview.closest("button") ?? preview);
    fireEvent.pointerMove(preview,{pointerType:"mouse"});
    expect((await screen.findAllByText("Sent by another agent · Review agent")).length).toBeGreaterThan(0);
  });
  it("lists the queue under a header that collapses it", () => {
    list();
    const header = screen.getByRole("button", { name: "Collapse queued messages" });
    expect(header.textContent).toContain("Queued");
    expect(header.textContent).toContain("3");
    expect(screen.getByRole("list", { name: "Queued messages" }).hidden).toBe(false);
    fireEvent.click(header);
    expect(screen.getByRole("list", { hidden: true, name: "Queued messages" }).className).toContain("hidden");
    screen.getByRole("button", { name: "Expand queued messages" });
  });

  it("offers edit, steer, interrupt and remove on a live turn", () => {
    const props = list();
    const [first] = rows();
    fireEvent.click(first.querySelector('[aria-label="Steer with queued message"]')!);
    expect(props.onSteer).toHaveBeenCalledWith("a");
    fireEvent.click(first.querySelector('[aria-label="Interrupt and send queued message"]')!);
    expect(props.onInterrupt).toHaveBeenCalledWith("a");
    fireEvent.click(first.querySelector('[aria-label="Remove queued message"]')!);
    expect(props.onRemove).toHaveBeenCalledWith("a");
    fireEvent.click(first.querySelector('[aria-label="Edit queued message"]')!);
    expect(props.editor.begin).toHaveBeenCalledWith(props.queued[0]);
  });

  it("hides Steer for a provider that cannot steer", () => {
    list({ canSteer: false });
    expect(screen.queryAllByRole("button", { name: "Steer with queued message" })).toHaveLength(0);
    expect(screen.getAllByRole("button", { name: "Interrupt and send queued message" })).toHaveLength(3);
  });

  it("moves a turn with the arrow keys on its grip", () => {
    list();
    const grips = screen.getAllByRole("button", { name: /Reorder queued message/ });
    fireEvent.keyDown(grips[0], { key: "ArrowDown" });
    expect(commands.agentChatMoveQueuedTurn).toHaveBeenLastCalledWith("claude", "t", "a", "c");
    fireEvent.keyDown(grips[2], { key: "ArrowUp" });
    expect(commands.agentChatMoveQueuedTurn).toHaveBeenLastCalledWith("claude", "t", "c", "b");
    fireEvent.keyDown(grips[1], { key: "ArrowDown" });
    expect(commands.agentChatMoveQueuedTurn).toHaveBeenLastCalledWith("claude", "t", "b", null);
    // Nothing above the first or below the last.
    commands.agentChatMoveQueuedTurn.mockClear();
    fireEvent.keyDown(grips[0], { key: "ArrowUp" });
    fireEvent.keyDown(grips[2], { key: "ArrowDown" });
    expect(commands.agentChatMoveQueuedTurn).not.toHaveBeenCalled();
  });

  it("gives a saved turn Send instead of Steer and Interrupt", () => {
    const props = list({ queued: [item("s", true)] });
    screen.getByText("Saved");
    expect(screen.queryByRole("button", { name: "Steer with queued message" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Interrupt and send queued message" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Send saved message" }));
    expect(props.requestSendAnchor).toHaveBeenCalledWith("nonce-s");
    expect(commands.agentChatSendSavedQueuedTurn).toHaveBeenCalledWith("claude", "t", "s", "queue");
    fireEvent.click(screen.getByRole("button", { name: "Remove queued message" }));
    expect(commands.agentChatCancelSavedQueuedTurn).toHaveBeenCalledWith("t", "s");
    expect(props.onRemove).not.toHaveBeenCalled();
  });

  it("cancels an edit on Escape from its own composer only", () => {
    const cancel = vi.fn();
    const props: QueuedTurnsListProps = {
      threadId: "t", provider: "claude", queued: [item("a")], editor: editor({ editingId: "a", cancel }),
      streaming: true, canSteer: true, onSteer: vi.fn(), onInterrupt: vi.fn(), onRemove: vi.fn(),
      requestSendAnchor: vi.fn(), clearSendAnchor: vi.fn(),
    };
    render(
      <TooltipProvider>
        <div data-testid="other-pane"><textarea data-composer-input aria-label="other" /></div>
        <div>
          <QueuedTurnsList {...props} />
          <textarea data-composer-input aria-label="mine" />
        </div>
      </TooltipProvider>,
    );
    fireEvent.keyDown(screen.getByRole("textbox", { name: "other" }), { key: "Escape" });
    expect(cancel).not.toHaveBeenCalled();
    fireEvent.keyDown(screen.getByRole("textbox", { name: "mine" }), { key: "Escape" });
    expect(cancel).toHaveBeenCalledTimes(1);
  });

  it("shows only Cancel on the row being edited", () => {
    const props = list({ editor: editor({ editingId: "b" }) });
    const editing = rows()[1];
    expect(editing.getAttribute("aria-current")).toBe("true");
    expect(editing.querySelector('[aria-label="Edit queued message"]')).toBeNull();
    expect(editing.querySelector('[aria-label="Remove queued message"]')).toBeNull();
    fireEvent.click(editing.querySelector('[aria-label="Cancel editing queued message"]')!);
    expect(props.editor.cancel).toHaveBeenCalled();
  });
});

describe("useQueuedTurnEditor", () => {
  const thread = "edit-thread";

  function seed(draft: string, queued: UserMessageItem[]) {
    act(() => {
      useAgentChatStore.getState().ensureThread(thread);
      useAgentChatStore.setState((state) => ({
        threads: {
          ...state.threads,
          [thread]: { ...state.threads[thread], inputDraft: draft, messages: queued },
        },
      }));
    });
  }

  const draft = () => useAgentChatStore.getState().threads[thread].inputDraft;

  it("loads the turn into the composer and puts the draft back on cancel", () => {
    seed("half-typed", [item("a")]);
    const { result } = renderHook(() => useQueuedTurnEditor({ threadId: thread, provider: "claude" }));
    act(() => result.current.begin(item("a")));
    expect(result.current.editingId).toBe("a");
    expect(draft()).toBe("text a");
    act(() => result.current.cancel());
    expect(draft()).toBe("half-typed");
  });

  it("lets an edit of a turn with images clear its words", () => {
    const withImage = { ...item("a"), images: [{ src: "/tmp/a.png", mediaType: "image/png" }] };
    seed("", [withImage]);
    const { result } = renderHook(() => useQueuedTurnEditor({ threadId: thread, provider: "claude" }));
    act(() => result.current.begin(item("a")));
    expect(result.current.editingHasImages).toBe(false);
    act(() => result.current.begin(withImage));
    expect(result.current.editingHasImages).toBe(true);
  });

  it("puts the displaced draft back when the pane goes away mid-edit", () => {
    seed("half-typed", [item("a")]);
    const { result, unmount } = renderHook(() => useQueuedTurnEditor({ threadId: thread, provider: "claude" }));
    act(() => result.current.begin(item("a")));
    unmount();
    expect(draft()).toBe("half-typed");
  });

  it("keeps what was typed while a save was in flight", async () => {
    seed("half-typed", [item("a")]);
    let resolve: (value: boolean) => void = () => {};
    commands.agentChatEditQueuedTurn.mockImplementation(() => new Promise((r) => { resolve = r; }));
    const { result } = renderHook(() => useQueuedTurnEditor({ threadId: thread, provider: "claude" }));
    act(() => result.current.begin(item("a")));
    let saving: Promise<void> = Promise.resolve();
    act(() => {
      saving = result.current.save("rewritten");
    });
    // A second submit while saving does nothing.
    await act(() => result.current.save("again"));
    expect(commands.agentChatEditQueuedTurn).toHaveBeenCalledTimes(1);
    act(() => useAgentChatStore.getState().setInputDraft(thread, "next message"));
    await act(async () => {
      resolve(true);
      await saving;
    });
    expect(result.current.editingId).toBeNull();
    // The displaced draft comes back above the newer typing.
    expect(draft()).toBe("half-typed\nnext message");
  });

  it("saves the composer text onto the turn instead of sending", async () => {
    seed("half-typed", [item("a")]);
    const { result } = renderHook(() => useQueuedTurnEditor({ threadId: thread, provider: "claude" }));
    act(() => result.current.begin(item("a")));
    act(() => useAgentChatStore.getState().setInputDraft(thread, "rewritten"));
    await act(() => result.current.save("rewritten"));
    expect(commands.agentChatEditQueuedTurn).toHaveBeenCalledWith("claude", thread, "a", "rewritten");
    expect(result.current.editingId).toBeNull();
    expect(draft()).toBe("half-typed");
  });

  it("keeps another turn's text when it is removed mid-edit", () => {
    seed("half-typed", [item("a"), item("b")]);
    const { result } = renderHook(() => useQueuedTurnEditor({ threadId: thread, provider: "claude" }));
    act(() => result.current.begin(item("a")));
    // Removing B hands its text back to the composer holding A's edit.
    act(() => {
      useAgentChatStore.getState().applyEvent(thread, {
        type: "queued_turn_cancelled",
        thread_id: thread,
        queued_id: "b",
      });
    });
    expect(draft()).toBe("text a");
    act(() => result.current.cancel());
    expect(draft()).toBe("half-typed\ntext b");
  });

  it("lets a pending save settle an edit whose turn dispatched meanwhile", async () => {
    seed("half-typed", [item("a")]);
    let resolve: (value: boolean) => void = () => {};
    commands.agentChatEditQueuedTurn.mockImplementation(() => new Promise((r) => { resolve = r; }));
    const { result } = renderHook(() => useQueuedTurnEditor({ threadId: thread, provider: "claude" }));
    act(() => result.current.begin(item("a")));
    act(() => useAgentChatStore.getState().setInputDraft(thread, "rewritten"));
    let saving: Promise<void> = Promise.resolve();
    act(() => {
      saving = result.current.save("rewritten");
    });
    // Dispatch promotes the bubble: it stays, no longer queued.
    seed("rewritten", [{ ...item("a"), queued: undefined }]);
    expect(result.current.editingId).toBe("a");
    await act(async () => {
      resolve(true);
      await saving;
    });
    expect(result.current.editingId).toBeNull();
    expect(draft()).toBe("half-typed");
  });

  it("keeps a changed draft, and the displaced one, when the turn dispatches mid-edit", () => {
    seed("half-typed", [item("a")]);
    const { result } = renderHook(() => useQueuedTurnEditor({ threadId: thread, provider: "claude" }));
    act(() => result.current.begin(item("a")));
    act(() => useAgentChatStore.getState().setInputDraft(thread, "my edit"));
    // Dispatch promotes the bubble: it stays, no longer queued.
    seed("my edit", [{ ...item("a"), queued: undefined }]);
    expect(result.current.editingId).toBeNull();
    expect(draft()).toBe("half-typed\nmy edit");
  });

  it("returns an edit once when its turn is cancelled during the save, either way the save ends", async () => {
    for (const accepted of [false, true]) {
      seed("D", [item("a")]);
      let resolve: (value: boolean) => void = () => {};
      commands.agentChatEditQueuedTurn.mockImplementation(() => new Promise((r) => { resolve = r; }));
      const hook = renderHook(() => useQueuedTurnEditor({ threadId: thread, provider: "claude" }));
      act(() => hook.result.current.begin(item("a")));
      act(() => useAgentChatStore.getState().setInputDraft(thread, "E"));
      let saving: Promise<void> = Promise.resolve();
      act(() => {
        saving = hook.result.current.save("E");
      });
      act(() => {
        const store = useAgentChatStore.getState();
        // An accepted edit reached the bubble before the cancellation did.
        if (accepted) store.applyEvent(thread, { type: "queued_turn_edited", thread_id: thread, queued_id: "a", text: "E" });
        store.applyEvent(thread, { type: "queued_turn_cancelled", thread_id: thread, queued_id: "a" });
      });
      await act(async () => {
        resolve(accepted);
        await saving;
      });
      expect(hook.result.current.editingId).toBeNull();
      expect(draft()).toBe("D\nE");
      hook.unmount();
    }
  });

  it("strips the whole edited copy when the edit keeps the original as its last line", async () => {
    const original = { ...item("a"), text: "run tests" };
    seed("D", [original]);
    let resolve: (value: boolean) => void = () => {};
    commands.agentChatEditQueuedTurn.mockImplementation(() => new Promise((r) => { resolve = r; }));
    const hook = renderHook(() => useQueuedTurnEditor({ threadId: thread, provider: "claude" }));
    act(() => hook.result.current.begin(original));
    act(() => useAgentChatStore.getState().setInputDraft(thread, "first lint\nrun tests"));
    let saving: Promise<void> = Promise.resolve();
    act(() => {
      saving = hook.result.current.save("first lint\nrun tests");
    });
    act(() => {
      const store = useAgentChatStore.getState();
      store.applyEvent(thread, { type: "queued_turn_edited", thread_id: thread, queued_id: "a", text: "first lint\nrun tests" });
      store.applyEvent(thread, { type: "queued_turn_cancelled", thread_id: thread, queued_id: "a" });
    });
    await act(async () => {
      resolve(true);
      await saving;
    });
    expect(draft()).toBe("D\nfirst lint\nrun tests");
  });

  it("keeps a repeated-line edit whole when the cancellation wins the save", async () => {
    const original = { ...item("a"), text: "run tests" };
    seed("D", [original]);
    let resolve: (value: boolean) => void = () => {};
    commands.agentChatEditQueuedTurn.mockImplementation(() => new Promise((r) => { resolve = r; }));
    const hook = renderHook(() => useQueuedTurnEditor({ threadId: thread, provider: "claude" }));
    act(() => hook.result.current.begin(original));
    act(() => useAgentChatStore.getState().setInputDraft(thread, "run tests\nrun tests"));
    let saving: Promise<void> = Promise.resolve();
    act(() => {
      saving = hook.result.current.save("run tests\nrun tests");
    });
    act(() => {
      useAgentChatStore.getState().applyEvent(thread, { type: "queued_turn_cancelled", thread_id: thread, queued_id: "a" });
    });
    await act(async () => {
      resolve(false);
      await saving;
    });
    expect(draft()).toBe("D\nrun tests\nrun tests");
  });

  it("returns the edited turn's words once when it is cancelled mid-edit", () => {
    const cancel = () =>
      act(() => {
        useAgentChatStore.getState().applyEvent(thread, {
          type: "queued_turn_cancelled",
          thread_id: thread,
          queued_id: "a",
        });
      });
    // Unchanged: the composer holds the displaced draft and the turn's text.
    seed("half-typed", [item("a")]);
    const first = renderHook(() => useQueuedTurnEditor({ threadId: thread, provider: "claude" }));
    act(() => first.result.current.begin(item("a")));
    cancel();
    expect(first.result.current.editingId).toBeNull();
    expect(draft()).toBe("half-typed\ntext a");
    first.unmount();
    // Edited: the edit is what returns, below the displaced draft.
    seed("half-typed", [item("a")]);
    const second = renderHook(() => useQueuedTurnEditor({ threadId: thread, provider: "claude" }));
    act(() => second.result.current.begin(item("a")));
    act(() => useAgentChatStore.getState().setInputDraft(thread, "my edit"));
    cancel();
    expect(draft()).toBe("half-typed\nmy edit");
  });
});
