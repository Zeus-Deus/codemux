import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";
import type { ChatViewItem } from "@/lib/agent-chat/types";

const trace = vi.hoisted(() => ({ enabled: true, ready: vi.fn() }));
vi.mock("@/lib/perf/interaction-trace", () => ({
  isInteractionTraceEnabled: () => trace.enabled,
  markPaneReady: trace.ready,
}));
vi.mock("./MessageList", () => ({
  MessageList: () => (
    <div data-slot="transcript-list">
      <div data-testid="virtual-rows" style={{ opacity: 0 }}>
        <div data-index="0">Newest answer</div>
      </div>
    </div>
  ),
}));
import { ChatTranscript } from "./ChatTranscript";
const messages: ChatViewItem[] = [
  { kind: "assistant_message", id: "a", seq: 1, text: "Newest answer", streaming: false, turn_id: "t" },
];
const props = {
  messages,
  streaming: false,
  workspaceId: "ws-a",
  threadKey: "thread-a",
  onRespondToRequest: vi.fn(),
  onAcceptPlan: vi.fn(),
  onRejectPlan: vi.fn(),
};
beforeEach(() => {
  vi.useFakeTimers();
  trace.enabled = true;
  trace.ready.mockClear();
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
    width: 400, height: 100, top: 20, bottom: 120, left: 0, right: 400,
  } as DOMRect);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it("reports the owning workspace only after virtualized content is visible", () => {
  const view = render(<ChatTranscript {...props} />);
  vi.advanceTimersByTime(100);
  expect(trace.ready).not.toHaveBeenCalled();
  view.getByTestId("virtual-rows").style.opacity = "1";
  vi.advanceTimersByTime(40);
  expect(trace.ready).toHaveBeenCalledExactlyOnceWith("agent-chat", { target: "ws-a" });
});

it("cancels the old thread's pending readiness on a thread/workspace change", () => {
  const view = render(<ChatTranscript {...props} />);
  view.getByTestId("virtual-rows").style.opacity = "1";
  vi.advanceTimersToNextFrame();
  view.rerender(<ChatTranscript {...props} workspaceId="ws-b" threadKey="thread-b" />);
  vi.advanceTimersByTime(40);
  expect(trace.ready).toHaveBeenCalledExactlyOnceWith("agent-chat", { target: "ws-b" });
});

it("does not poll DOM on normal navigation with tracing disabled", () => {
  trace.enabled = false;
  render(<ChatTranscript {...props} />);
  expect(vi.getTimerCount()).toBe(0);
  expect(trace.ready).not.toHaveBeenCalled();
});

it("does not declare an empty transcript text-ready", () => {
  render(<ChatTranscript {...props} messages={[]} />);
  expect(vi.getTimerCount()).toBe(0);
  expect(trace.ready).not.toHaveBeenCalled();
});
