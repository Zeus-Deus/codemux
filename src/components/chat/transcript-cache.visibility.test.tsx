import { cleanup, render, act } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ChatTranscript } from "./ChatTranscript";
import { TranscriptCacheProvider } from "./transcript-cache";
import { TranscriptBindingContext } from "./transcript-cache-binding";
const trace = vi.hoisted(() => ({ enabled: true, ready: vi.fn() }));
vi.mock("@/lib/perf/interaction-trace", () => ({ isInteractionTraceEnabled: () => trace.enabled, markPaneReady: trace.ready }));
vi.mock("./MessageList", () => ({ MessageList: ({ workspaceId }: { workspaceId: string }) =>
  <div data-slot="transcript-list"><div data-testid={workspaceId} style={{ opacity: 0 }}><div data-index="0">answer</div></div></div>,
}));
beforeEach(() => {
  vi.useFakeTimers(); trace.enabled = true; trace.ready.mockClear();
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({ width: 400, height: 100, top: 20, bottom: 120, left: 0, right: 400 } as DOMRect);
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.useRealTimers(); });
function tree(id: string) {
  return <TranscriptCacheProvider activeKey={id} validKeys={["a", "b"]}>
    <TranscriptBindingContext.Provider value={{ key: id, workspaceId: id, threadKey: id, provider: "claude", cwd: "/project" }}>
      <ChatTranscript key={id} workspaceId={id} threadKey={id} provider="claude" cwd="/project" streaming={false}
        messages={[{ kind: "assistant_message", id, seq: 1, text: id, turn_id: "t", streaming: false }]}
        onRespondToRequest={vi.fn()} onAcceptPlan={vi.fn()} onRejectPlan={vi.fn()} />
    </TranscriptBindingContext.Provider>
  </TranscriptCacheProvider>;
}
it("cancels outgoing probes, ignores parked rows, and marks the freshly visible cache slot", () => {
  const view = render(tree("a"));
  const a = view.getByTestId("a");
  a.style.opacity = "1";
  act(() => vi.advanceTimersToNextFrame());
  view.rerender(tree("b"));
  act(() => vi.advanceTimersByTime(80));
  expect(trace.ready).not.toHaveBeenCalled();
  view.getByTestId("b").style.opacity = "1";
  act(() => vi.advanceTimersByTime(40));
  expect(trace.ready).toHaveBeenCalledExactlyOnceWith("agent-chat", { target: "b" });
  trace.ready.mockClear();
  view.rerender(tree("a"));
  expect(view.getByTestId("a")).toBe(a);
  expect(trace.ready).not.toHaveBeenCalled();
  act(() => vi.advanceTimersByTime(40));
  expect(trace.ready).toHaveBeenCalledExactlyOnceWith("agent-chat", { target: "a" });
  expect(vi.getTimerCount()).toBe(0);
});
it("does not poll cache DOM with tracing disabled", () => {
  trace.enabled = false;
  const view = render(tree("a"));
  view.rerender(tree("b"));
  view.rerender(tree("a"));
  expect(vi.getTimerCount()).toBe(0);
  expect(trace.ready).not.toHaveBeenCalled();
});
