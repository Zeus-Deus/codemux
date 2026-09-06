import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { ChatTranscript } from "./ChatTranscript";
import { TranscriptCacheProvider } from "./transcript-cache";
import { TranscriptBindingContext, type TranscriptBinding } from "./transcript-cache-binding";
import type { ChatViewItem } from "@/lib/agent-chat/types";

const intent = vi.hoisted(() => ({ observe: vi.fn(), fail: false }));
vi.mock("@/stores/provider-runtime-intent-store", () => ({ useProviderRuntimeIntent: Object.assign(() => intent.observe, { getState: () => intent }) }));
vi.mock("@/lib/perf/interaction-trace", () => ({ isInteractionTraceEnabled: () => false }));
vi.mock("./MessageList", () => ({ MessageList: ({ messages, onAcceptPlan }: { messages: ChatViewItem[]; onAcceptPlan: (id: string) => void }) => {
  if (intent.fail) throw Error("broken row");
  return <div data-slot="transcript-list"><button data-index="0" onClick={() => onAcceptPlan("request")}>{messages[0].id}</button></div>; },
}));
afterEach(() => { cleanup(); intent.fail = false; vi.restoreAllMocks(); });
it("keeps transcript render errors inside a local recoverable boundary", () => {
  intent.fail = true;
  vi.spyOn(console, "error").mockImplementation(() => {});
  expect(() => render(<Harness id="a" />)).not.toThrow();
  expect(document.querySelector('[role="alert"]')?.textContent).toContain("Couldn’t load agent chat");
});
it("keeps provider runtime intent on cached transcript interactions despite portal ancestry", () => {
  intent.observe.mockClear();
  const view = render(<Harness id="a" />);
  fireEvent.pointerDown(view.getByText("a"));
  fireEvent.keyDown(view.getByText("a"), { key: "Enter" });
  fireEvent.focus(view.getByText("a"));
  expect(intent.observe).toHaveBeenCalledTimes(3);
  expect(intent.observe).toHaveBeenLastCalledWith("claude");
});
it("falls back for a transient local thread mismatch without caching its DOM", () => {
  const view = render(<Harness id="a" mismatch />);
  const row = view.getByText("a");
  expect(row.closest('[data-transcript-cache-host]')).toBeNull();
  view.rerender(<Harness id="b" />);
  view.rerender(<Harness id="a" mismatch />);
  expect(view.getByText("a")).not.toBe(row);
});
it("supports an unbound standalone transcript", () => {
  const view = render(<ChatTranscript {...props("a")} workspaceId={undefined} />);
  expect(view.getByText("a")).toBeTruthy();
});
const binding = (id: string): TranscriptBinding => ({ key: id, workspaceId: id, threadKey: `thread-${id}`, cwd: "/project", provider: "claude" });
const props = (id: string) => ({ messages: [{ kind: "assistant_message", id, seq: 1, text: id, streaming: false, turn_id: "t" }] as ChatViewItem[], streaming: false, workspaceId: id, threadKey: `thread-${id}`, cwd: "/project", provider: "claude" as const, onRespondToRequest: vi.fn(), onAcceptPlan: vi.fn(), onRejectPlan: vi.fn() });
function Harness({ id, callback = vi.fn(), mismatch = false }: { id: string; callback?: (id: string) => void; mismatch?: boolean }) {
  return <TranscriptCacheProvider activeKey={id} validKeys={["a", "b"]}>
    <TranscriptBindingContext.Provider value={binding(id)}>
      <ChatTranscript key={id} {...props(id)} threadKey={mismatch ? "other" : `thread-${id}`} onAcceptPlan={callback} />
    </TranscriptBindingContext.Provider>
  </TranscriptCacheProvider>;
}
it("leases only presentation and updates callbacks on cache reveal", () => {
  const old = vi.fn();
  const current = vi.fn();
  const view = render(<Harness id="a" callback={old} />);
  const row = view.getByText("a");
  view.rerender(<Harness id="b" />);
  view.rerender(<Harness id="a" callback={current} />);
  expect(view.getByText("a")).toBe(row);
  fireEvent.click(row);
  expect(current).toHaveBeenCalledExactlyOnceWith("request");
  expect(old).not.toHaveBeenCalled();
});
