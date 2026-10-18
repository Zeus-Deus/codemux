import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";

import type { AgentChatEventPayload } from "@/tauri/events";

const { listen, unlisten, errorToast } = vi.hoisted(() => ({
  listen: vi.fn(),
  unlisten: vi.fn(),
  errorToast: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({ listen, emit: vi.fn() }));
vi.mock("@/lib/toast", () => ({ toast: { error: errorToast } }));

import { useAgentChatWarnings } from "./use-agent-chat-warnings";

beforeEach(() => {
  vi.clearAllMocks();
  listen.mockResolvedValue(unlisten);
});

function deliver(payload: AgentChatEventPayload) {
  const handler = listen.mock.calls.find(
    ([name]) => name === "agent_chat_event",
  )?.[1];
  expect(handler).toBeTypeOf("function");
  act(() => handler({ payload }));
}

it.each([
  ["provider_event_lag", "Provider events were lost"],
  ["transcript_write_failed", "Couldn't save conversation history"],
])("surfaces %s without requiring an attached chat pane", (code, title) => {
  renderHook(() => useAgentChatWarnings());
  deliver({
    thread_id: "",
    event: {
      type: "runtime_warning",
      thread_id: null,
      message: "Conversation history may be incomplete.",
      original_payload: { code },
    },
  });
  expect(errorToast).toHaveBeenCalledWith(title, {
    id: `agent-chat-${code}`,
    description: "Conversation history may be incomplete.",
  });
});

it("keeps ordinary SDK diagnostics and other events out of error toasts", () => {
  renderHook(() => useAgentChatWarnings());
  for (const original_payload of [null, "noise", {}, { code: "sdk_debug" }]) {
    deliver({
      thread_id: "",
      event: {
        type: "runtime_warning",
        thread_id: null,
        message: "SDK debug",
        original_payload,
      },
    });
  }
  deliver({
    thread_id: "t",
    event: {
      type: "session_state_changed",
      thread_id: "t",
      status: { status: "ready" },
    },
  });
  expect(errorToast).not.toHaveBeenCalled();
});

it("removes the global event listener on unmount", async () => {
  const { unmount } = renderHook(() => useAgentChatWarnings());
  await waitFor(() => expect(listen).toHaveBeenCalledTimes(1));
  unmount();
  expect(unlisten).toHaveBeenCalledTimes(1);
});
