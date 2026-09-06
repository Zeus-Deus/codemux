/// <reference types="@testing-library/jest-dom/vitest" />
import { Activity, type ComponentProps } from "react";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import type { ChatViewItem } from "@/lib/agent-chat/types";
import { getTitlebarContentUnder, getTitlebarTranscriptElements } from "@/lib/titlebar-content-under";
import { MessageList } from "./MessageList";

// Real MessageList, real rows, real LegendList. Only the missing browser
// layout/scroll APIs are supplied: this is not the virtualizer test double.
const handlers = {
  onRespondToRequest: vi.fn(),
  onAcceptPlan: vi.fn(),
  onRejectPlan: vi.fn(),
};
const messages: ChatViewItem[] = [
  { kind: "user_message", id: "u", seq: 0, text: "Inspect", created_at: 1000 },
  { kind: "assistant_message", id: "a", seq: 1, text: "Earlier explanation", turn_id: "t", streaming: false },
  { kind: "assistant_message", id: "b", seq: 2, text: "Final answer", turn_id: "t", streaming: false },
  { kind: "turn_ended", id: "e", seq: 3, turn_id: "t", status: { kind: "success" }, completed_at: 6000 },
];
type Props = ComponentProps<typeof MessageList>;
function tree(mode: "visible" | "hidden", props: Partial<Props> = {}) {
  return <Activity mode={mode}><MessageList messages={messages} threadKey="one" {...handlers} {...props} /></Activity>;
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
      if (size !== previous) {
        this.targets.set(target, size);
        entries.push({ target, contentRect: rect } as ResizeObserverEntry);
      }
    }
    if (entries.length) this.callback(entries, this as unknown as ResizeObserver);
  }
}
async function frames(count = 5) {
  for (let i = 0; i < count; i++) {
    await act(async () => { await vi.advanceTimersByTimeAsync(16); });
    act(() => { for (const observer of observers) observer.flush(); });
  }
}
function heightOf(element: HTMLElement): number {
  if (element.dataset.slot === "transcript-list") return 500;
  if (element.style.height.endsWith("px")) return parseFloat(element.style.height);
  if (element.classList.contains("legend-list-content-container")) {
    return Array.from(element.children).reduce((sum, child) => sum + heightOf(child as HTMLElement), 0);
  }
  return 100;
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
  vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(function (this: HTMLElement) {
    return Math.max(500, heightOf(this.querySelector<HTMLElement>(".legend-list-content-container") ?? this));
  });
  const offsets = new WeakMap<HTMLElement, number>();
  vi.spyOn(HTMLElement.prototype, "scrollTop", "get").mockImplementation(function (this: HTMLElement) {
    const offset = offsets.get(this) ?? 0;
    const clamped = this.dataset.slot === "transcript-list" ? Math.min(offset, Math.max(0, this.scrollHeight - this.clientHeight)) : offset;
    if (offset !== clamped) {
      offsets.set(this, clamped);
      window.setTimeout(() => this.dispatchEvent(new Event("scroll")), 0);
    }
    return clamped;
  });
  vi.spyOn(HTMLElement.prototype, "scrollTop", "set").mockImplementation(function (this: HTMLElement, offset: number) { offsets.set(this, offset); });
  vi.stubGlobal("ResizeObserver", GeometryObserver);
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => window.setTimeout(() => cb(performance.now()), 16));
  vi.stubGlobal("cancelAnimationFrame", (id: number) => window.clearTimeout(id));
  Object.defineProperty(HTMLElement.prototype, "scrollBy", { configurable: true, value: function (options: ScrollToOptions) {
    this.scrollTo({ top: this.scrollTop + (options.top ?? 0) });
  } });
  Object.defineProperty(HTMLElement.prototype, "scrollTo", { configurable: true, value: function (options: ScrollToOptions) {
    this.scrollTop = Math.max(0, Math.min(options.top ?? this.scrollTop, this.scrollHeight - this.clientHeight));
    window.setTimeout(() => this.dispatchEvent(new Event("scroll")), 0);
  } });
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  delete (HTMLElement.prototype as { scrollTo?: unknown }).scrollTo;
  delete (HTMLElement.prototype as { scrollBy?: unknown }).scrollBy;
});

it("unregisters the retained viewport while hidden and reattaches live titlebar events", async () => {
  const props = { workspaceId: "lifecycle-titlebar" };
  const view = render(tree("visible", props));
  await frames();
  const viewport = view.container.querySelector<HTMLElement>('[data-slot="transcript-list"]')!;
  for (let i = 0; i < 3; i++) {
    expect(getTitlebarTranscriptElements()).toContain(viewport);
    view.rerender(tree("hidden", props));
    expect(getTitlebarTranscriptElements()).not.toContain(viewport);
    expect(getTitlebarContentUnder(props.workspaceId)).toBe(false);
    view.rerender(tree("visible", props));
    await frames();
    expect(getTitlebarTranscriptElements().filter((node) => node === viewport)).toHaveLength(1);
    viewport.scrollTop = 0;
    fireEvent.scroll(viewport);
    expect(getTitlebarContentUnder(props.workspaceId)).toBe(false);
  }
  view.unmount();
  expect(getTitlebarTranscriptElements()).not.toContain(viewport);
  expect(getTitlebarContentUnder(props.workspaceId)).toBe(false);
});

it("uses the fresh pane's row callbacks and does not take focus on reveal", async () => {
  const oldAction = vi.fn();
  const newAction = vi.fn();
  const oldProps = {
    messages: [{ kind: "user_message", id: "queued", seq: 1, text: "Queued prompt", queued: { queuedId: "q" } }] as ChatViewItem[],
    onCancelQueued: oldAction,
    positionedNonceRef: { current: null as number | null },
    sendAnchor: null,
  };
  const view = render(tree("visible", oldProps));
  await frames();
  const button = screen.getByRole("button", { name: "Cancel queued message" });
  view.rerender(tree("hidden", oldProps));
  const outside = document.createElement("button");
  document.body.append(outside);
  outside.focus();
  view.rerender(tree("visible", { ...oldProps, positionedNonceRef: { current: null }, onCancelQueued: newAction }));
  await frames();
  expect(document.activeElement).toBe(outside);
  expect(screen.getByRole("button", { name: "Cancel queued message" })).toBe(button);
  fireEvent.click(button);
  expect(oldAction).not.toHaveBeenCalled();
  expect(newAction).toHaveBeenCalledExactlyOnceWith("q", "Queued prompt");
  outside.remove();
});

it("restores a pending jump pill after reconnecting away from the live edge", async () => {
  const history: ChatViewItem[] = Array.from({ length: 30 }, (_, seq) => ({ kind: "user_message", id: `pill-${seq}`, seq, text: `prompt ${seq}` }));
  const props = { messages: history };
  const view = render(tree("visible", props));
  await frames(60);
  const viewport = view.container.querySelector<HTMLElement>('[data-slot="transcript-list"]')!;
  // A wheel gesture produces successive offsets: the first clears the
  // virtualizer's pending footer target, the next retires its bottom target.
  fireEvent.wheel(viewport, { deltaY: -100 });
  viewport.scrollTop = 600;
  fireEvent.scroll(viewport);
  await frames(2);
  fireEvent.wheel(viewport, { deltaY: -200 });
  viewport.scrollTop = 400;
  fireEvent.scroll(viewport);
  await frames(5);
  expect(viewport.scrollTop).toBe(400);
  view.rerender(tree("hidden", props));
  await frames(20);
  view.rerender(tree("visible", props));
  await frames(20);
  expect(viewport.scrollTop).toBe(400);
  expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeInTheDocument();
});

it("positions an unissued send after hiding before its first animation frame", async () => {
  const positionedNonceRef = { current: null as number | null };
  const props = {
    messages: [{ kind: "user_message", id: "pending", seq: 1, text: "Pending prompt", clientNonce: "pending" }] as ChatViewItem[],
    positionedNonceRef,
    sendAnchor: { nonce: 2, clientNonce: "pending" },
  };
  const view = render(tree("visible", props));
  expect(positionedNonceRef.current).toBeNull();
  view.rerender(tree("hidden", props));
  await frames(60);
  expect(positionedNonceRef.current).toBeNull();
  view.rerender(tree("visible", props));
  await frames(60);
  expect(positionedNonceRef.current).toBe(2);
});

it("releases a canceled send glide so a revealed answer can keep advancing", async () => {
  const history: ChatViewItem[] = Array.from({ length: 3 }, (_, seq) => ({
    kind: "user_message", id: `u-${seq}`, seq, text: `prompt ${seq}`,
  }));
  const positionedNonceRef = { current: null as number | null };
  const view = render(tree("visible", { messages: history, positionedNonceRef }));
  await frames(20);
  const prompt: ChatViewItem = { kind: "user_message", id: "sent", seq: 20, text: "New prompt", clientNonce: "sent" };
  const sent = [...history, prompt];
  const props = { messages: sent, positionedNonceRef, sendAnchor: { nonce: 1, clientNonce: "sent" } };
  view.rerender(tree("visible", props));
  await frames(20);
  expect(positionedNonceRef.current).toBe(1);
  const viewport = view.container.querySelector<HTMLElement>('[data-slot="transcript-list"]')!;
  const parkedOffset = viewport.scrollTop;
  view.rerender(tree("hidden", props));
  await frames(60); // the old 750ms fallback must not survive hiding
  expect(viewport.scrollTop).toBe(parkedOffset);
  view.rerender(tree("visible", props));
  await frames(5);
  const answer: ChatViewItem[] = Array.from({ length: 8 }, (_, i) => ({
    kind: "runtime_notice", id: `r-${i}`, seq: 21 + i, severity: "warning", message: `Answer ${i}`,
  }));
  view.rerender(tree("visible", { ...props, messages: [...sent, ...answer] }));
  await frames(60);
  expect(viewport.scrollTop).toBeGreaterThan(parkedOffset);
});

it("keeps the reader's offset when the retained virtualizer reconnects", async () => {
  const history: ChatViewItem[] = Array.from({ length: 30 }, (_, seq) => ({
    kind: "user_message", id: `u-${seq}`, seq, text: `prompt ${seq}`,
  }));
  const view = render(tree("visible", { messages: history }));
  await frames(20);
  const viewport = view.container.querySelector<HTMLElement>('[data-slot="transcript-list"]')!;
  fireEvent.wheel(viewport, { deltaY: -100 });
  viewport.scrollTop = 400;
  fireEvent.scroll(viewport);
  await frames(5);
  expect(viewport.scrollTop).toBe(400);
  view.rerender(tree("hidden", { messages: history }));
  await frames(5);
  view.rerender(tree("visible", { messages: history }));
  await frames(5);
  expect(view.container.querySelector('[data-slot="transcript-list"]')).toBe(viewport);
  expect(viewport.scrollTop).toBe(400);
});

it("preserves an expanded turn across effect disconnection, but resets on thread change", async () => {
  const view = render(tree("visible"));
  await frames();
  const fold = screen.getByRole("button", { name: "Worked for 5s" });
  fireEvent.click(fold);
  await frames();
  expect(fold).toHaveAttribute("aria-expanded", "true");
  const viewport = view.container.querySelector('[data-slot="transcript-list"]');
  view.rerender(tree("hidden"));
  await frames();
  view.rerender(tree("visible"));
  await frames();
  expect(view.container.querySelector('[data-slot="transcript-list"]')).toBe(viewport);
  expect(screen.getByRole("button", { name: "Worked for 5s" })).toHaveAttribute("aria-expanded", "true");
  view.rerender(tree("visible", { threadKey: "two" }));
  await frames();
  expect(screen.getByRole("button", { name: "Worked for 5s" })).toHaveAttribute("aria-expanded", "false");
});
