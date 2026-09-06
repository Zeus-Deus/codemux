import { StrictMode } from "react";
import { act, cleanup, fireEvent, render, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ChatTranscript } from "./ChatTranscript";
import { TranscriptCacheProvider } from "./transcript-cache";
import { TranscriptBindingContext } from "./transcript-cache-binding";
import type { ChatViewItem } from "@/lib/agent-chat/types";

// Real ChatTranscript, MessageList, row rendering AND LegendList. jsdom only
// needs a deterministic browser layout/scroll adapter; no renderer mocks.
function heightOf(element: HTMLElement): number {
  if (element.dataset.slot === "transcript-list") return 500;
  if (element.style.height.endsWith("px")) return parseFloat(element.style.height);
  if (element.classList.contains("legend-list-content-container")) return Array.from(element.children).reduce((sum, child) => sum + heightOf(child as HTMLElement), 0);
  return 100;
}
const observers = new Set<GeometryObserver>();
class GeometryObserver {
  private targets = new Map<Element, string>();
  constructor(private callback: ResizeObserverCallback) { observers.add(this); }
  observe(target: Element) { this.targets.set(target, ""); }
  unobserve(target: Element) { this.targets.delete(target); }
  disconnect() { this.targets.clear(); observers.delete(this); }
  flush() {
    const entries: ResizeObserverEntry[] = [];
    for (const [target, previous] of this.targets) {
      const rect = target.getBoundingClientRect();
      const size = `${rect.width}:${rect.height}`;
      if (size !== previous) { this.targets.set(target, size); entries.push({ target, contentRect: rect } as ResizeObserverEntry); }
    }
    if (entries.length) this.callback(entries, this as unknown as ResizeObserver);
  }
}
beforeEach(() => {
  vi.useFakeTimers();
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    const height = heightOf(this);
    const viewport = this.closest<HTMLElement>('[data-slot="transcript-list"]');
    const row = this.closest<HTMLElement>('[data-index]');
    const top = row ? 100 + parseFloat(row.style.top || "0") - (viewport?.scrollTop ?? 0) : 0;
    return { x: 0, y: top, top, left: 0, bottom: top + height, right: 800, width: 800, height, toJSON() {} };
  });
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(500);
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(800);
  vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(800);
  vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(function (this: HTMLElement) { return Math.max(500, heightOf(this.querySelector<HTMLElement>(".legend-list-content-container") ?? this)); });
  // Browser scrollTop assignment queues a scroll event, including cache
  // restoration. jsdom's silent plain property otherwise leaves LegendList's
  // observed offset stale even though the DOM assertion reads the new value.
  const offsets = new WeakMap<HTMLElement, number>();
  vi.spyOn(HTMLElement.prototype, "scrollTop", "get").mockImplementation(function (this: HTMLElement) { return offsets.get(this) ?? 0; });
  vi.spyOn(HTMLElement.prototype, "scrollTop", "set").mockImplementation(function (this: HTMLElement, value: number) {
    const next = this.dataset.slot === "transcript-list" ? Math.max(0, Math.min(value, this.scrollHeight - this.clientHeight)) : value;
    if (next !== (offsets.get(this) ?? 0)) {
      offsets.set(this, next);
      window.setTimeout(() => this.dispatchEvent(new Event("scroll")), 0);
    }
  });
  vi.stubGlobal("ResizeObserver", GeometryObserver);
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => window.setTimeout(() => cb(performance.now()), 16));
  vi.stubGlobal("cancelAnimationFrame", (id: number) => window.clearTimeout(id));
  Object.defineProperty(HTMLElement.prototype, "scrollTo", { configurable: true, value: function (options: ScrollToOptions) {
    this.scrollTop = Math.max(0, Math.min(options.top ?? this.scrollTop, this.scrollHeight - this.clientHeight));
  } });
  Object.defineProperty(HTMLElement.prototype, "scrollBy", { configurable: true, value: function (options: ScrollToOptions) { this.scrollTo({ top: this.scrollTop + (options.top ?? 0) }); } });
});
afterEach(() => {
  cleanup(); vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals();
  delete (HTMLElement.prototype as { scrollTo?: unknown }).scrollTo;
  delete (HTMLElement.prototype as { scrollBy?: unknown }).scrollBy;
});
async function frames(count = 20) {
  for (let i = 0; i < count; i++) {
    await act(async () => { await vi.advanceTimersByTimeAsync(16); });
    act(() => { for (const observer of observers) observer.flush(); });
  }
}
const folded: ChatViewItem[] = [
  { kind: "user_message", id: "u", seq: 0, text: "Inspect", created_at: 1000 },
  { kind: "assistant_message", id: "a", seq: 1, text: "Earlier explanation", turn_id: "t", streaming: false },
  { kind: "assistant_message", id: "b", seq: 2, text: "Final answer", turn_id: "t", streaming: false },
  { kind: "turn_ended", id: "e", seq: 3, turn_id: "t", status: { kind: "success" }, completed_at: 6000 },
];
const callbacks = { onRespondToRequest: vi.fn(), onAcceptPlan: vi.fn(), onRejectPlan: vi.fn() };
function tree(id: string, messages = folded) {
  return <StrictMode><TranscriptCacheProvider activeKey={id} validKeys={["a", "b"]}>
    <TranscriptBindingContext.Provider value={{ key: id, workspaceId: id, threadKey: id, provider: "claude", cwd: "/project" }}>
      <section key={id} data-testid="pane"><textarea data-testid="composer" />
        <ChatTranscript messages={messages} threadKey={id} workspaceId={id} provider="claude" cwd="/project"
          positionedNonceRef={{ current: null }} sendAnchor={null} streaming={false} {...callbacks} />
      </section>
    </TranscriptBindingContext.Provider>
  </TranscriptCacheProvider></StrictMode>;
}
it("reparents the real virtualizer and preserves row DOM and expanded disclosure with a new pane", async () => {
  const view = render(tree("a"));
  await frames();
  const pane = view.getByTestId("pane");
  const fold = within(pane).getByRole("button", { name: "Worked for 5s" });
  fireEvent.click(fold);
  await frames();
  const row = within(pane).getByText("Earlier explanation");
  const viewport = pane.querySelector('[data-slot="transcript-list"]');
  const composer = view.getByTestId("composer");
  view.rerender(tree("b"));
  await frames();
  expect(viewport?.closest('[data-transcript-cache-parking]')).not.toBeNull();
  view.rerender(tree("a"));
  await frames();
  expect(view.getByTestId("pane")).not.toBe(pane);
  expect(view.getByTestId("composer")).not.toBe(composer);
  expect(view.getByTestId("pane").querySelector('[data-slot="transcript-list"]')).toBe(viewport);
  expect(within(view.getByTestId("pane")).getByText("Earlier explanation")).toBe(row);
  expect(fold.getAttribute("aria-expanded")).toBe("true");
});
it("keeps a reader's offset through host parking while the pane-owned refs are replaced", async () => {
  const messages: ChatViewItem[] = Array.from({ length: 30 }, (_, seq) => ({ kind: "user_message", id: `u-${seq}`, seq, text: `prompt ${seq}` }));
  const view = render(tree("a", messages));
  await frames(40);
  const viewport = view.getByTestId("pane").querySelector<HTMLElement>('[data-slot="transcript-list"]')!;
  // A real wheel gesture produces successive changed offsets. LegendList
  // clears its pending initial footer target on the first movement, then its
  // retained bottom target on the next; one jsdom teleport is not that gesture.
  fireEvent.wheel(viewport, { deltaY: -100 });
  viewport.scrollTop = 600;
  fireEvent.scroll(viewport);
  await frames(2);
  fireEvent.wheel(viewport, { deltaY: -200 });
  viewport.scrollTop = 400;
  fireEvent.scroll(viewport);
  await frames(5);
  expect(viewport.scrollTop).toBe(400);
  view.rerender(tree("b"));
  await frames(20);
  view.rerender(tree("a", messages));
  await frames(20);
  expect(view.getByTestId("pane").querySelector('[data-slot="transcript-list"]')).toBe(viewport);
  expect(viewport.scrollTop).toBe(400);
});
