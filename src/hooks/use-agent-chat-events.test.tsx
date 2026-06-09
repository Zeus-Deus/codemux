/// <reference types="@testing-library/jest-dom/vitest" />
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";

import type { AgentChatEventPayload } from "@/tauri/events";

/**
 * Fake of the `@tauri-apps/api` Channel re-exported by
 * `@/tauri/commands`: captures the onmessage handler so tests can
 * push payloads as if the backend had routed events to this channel.
 * Built inside `vi.hoisted` because `vi.mock` factories are hoisted
 * above ordinary top-level declarations.
 */
const { FakeChannel, attachAgentChatOutput, detachAgentChatOutput } =
  vi.hoisted(() => {
    class FakeChannel<T> {
      static instances: FakeChannel<unknown>[] = [];
      onmessage: (payload: T) => void;
      constructor(onmessage: (payload: T) => void) {
        this.onmessage = onmessage;
        FakeChannel.instances.push(this as FakeChannel<unknown>);
      }
    }
    return {
      FakeChannel,
      attachAgentChatOutput: vi.fn(),
      detachAgentChatOutput: vi.fn(),
    };
  });

type FakeChannel<T> = InstanceType<typeof FakeChannel<T>>;

vi.mock("@/tauri/commands", () => ({
  Channel: FakeChannel,
  attachAgentChatOutput,
  detachAgentChatOutput,
}));

import { useAgentChatEvents } from "./use-agent-chat-events";

function payloadFor(threadId: string, text: string): AgentChatEventPayload {
  return {
    thread_id: threadId,
    event: {
      type: "content_delta",
      thread_id: threadId,
      turn_id: "turn-1",
      delta: { kind: "text", text },
    },
  };
}

function lastChannel(): FakeChannel<AgentChatEventPayload> {
  const channel = FakeChannel.instances[FakeChannel.instances.length - 1];
  if (!channel) throw new Error("no channel constructed");
  return channel as FakeChannel<AgentChatEventPayload>;
}

describe("useAgentChatEvents", () => {
  beforeEach(() => {
    FakeChannel.instances = [];
    attachAgentChatOutput.mockReset().mockResolvedValue(7);
    detachAgentChatOutput.mockReset().mockResolvedValue(undefined);
  });

  it("does not attach when threadId is null", () => {
    renderHook(() => useAgentChatEvents(null, vi.fn()));
    expect(attachAgentChatOutput).not.toHaveBeenCalled();
    expect(FakeChannel.instances).toHaveLength(0);
  });

  it("attaches a channel for the thread and dispatches payloads to the handler", async () => {
    const handler = vi.fn();
    renderHook(() => useAgentChatEvents("thread-1", handler));

    expect(attachAgentChatOutput).toHaveBeenCalledTimes(1);
    expect(attachAgentChatOutput).toHaveBeenCalledWith(
      "thread-1",
      expect.any(FakeChannel),
    );

    const payload = payloadFor("thread-1", "tok");
    act(() => lastChannel().onmessage(payload));
    expect(handler).toHaveBeenCalledTimes(1);
    expect(handler).toHaveBeenCalledWith(payload);
  });

  it("filters payloads for a different thread (defense in depth)", () => {
    const handler = vi.fn();
    renderHook(() => useAgentChatEvents("thread-1", handler));

    act(() => lastChannel().onmessage(payloadFor("thread-other", "tok")));
    expect(handler).not.toHaveBeenCalled();
  });

  it("detaches with the attach generation on unmount and stops dispatching", async () => {
    const handler = vi.fn();
    const { unmount } = renderHook(() =>
      useAgentChatEvents("thread-1", handler),
    );
    const channel = lastChannel();

    unmount();
    await waitFor(() =>
      expect(detachAgentChatOutput).toHaveBeenCalledWith("thread-1", 7),
    );

    // Events racing the async detach are dropped client-side too.
    channel.onmessage(payloadFor("thread-1", "late"));
    expect(handler).not.toHaveBeenCalled();
  });

  it("does not re-attach when only the handler identity changes", () => {
    const { rerender } = renderHook(
      ({ handler }: { handler: (p: AgentChatEventPayload) => void }) =>
        useAgentChatEvents("thread-1", handler),
      { initialProps: { handler: vi.fn() } },
    );
    rerender({ handler: vi.fn() });
    expect(attachAgentChatOutput).toHaveBeenCalledTimes(1);
  });

  it("uses the latest handler without re-attaching", () => {
    const first = vi.fn();
    const second = vi.fn();
    const { rerender } = renderHook(
      ({ handler }: { handler: (p: AgentChatEventPayload) => void }) =>
        useAgentChatEvents("thread-1", handler),
      { initialProps: { handler: first } },
    );
    rerender({ handler: second });

    act(() => lastChannel().onmessage(payloadFor("thread-1", "tok")));
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledTimes(1);
  });

  it("re-attaches when the thread id changes and detaches the old thread", async () => {
    const handler = vi.fn();
    const { rerender } = renderHook(
      ({ threadId }: { threadId: string }) =>
        useAgentChatEvents(threadId, handler),
      { initialProps: { threadId: "thread-1" } },
    );
    rerender({ threadId: "thread-2" });

    await waitFor(() =>
      expect(detachAgentChatOutput).toHaveBeenCalledWith("thread-1", 7),
    );
    expect(attachAgentChatOutput).toHaveBeenCalledTimes(2);
    expect(attachAgentChatOutput).toHaveBeenLastCalledWith(
      "thread-2",
      expect.any(FakeChannel),
    );

    act(() => lastChannel().onmessage(payloadFor("thread-2", "tok")));
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it("skips detach when attach failed", async () => {
    attachAgentChatOutput.mockRejectedValue(new Error("backend gone"));
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    const { unmount } = renderHook(() =>
      useAgentChatEvents("thread-1", vi.fn()),
    );
    await waitFor(() => expect(consoleError).toHaveBeenCalled());
    unmount();
    // Let the rejected-detach microtask settle; detach must not fire.
    await act(async () => {});
    expect(detachAgentChatOutput).not.toHaveBeenCalled();
    consoleError.mockRestore();
  });
});
