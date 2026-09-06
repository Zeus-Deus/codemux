import { StrictMode, useEffect, useLayoutEffect, useRef, useSyncExternalStore } from "react";
import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { TranscriptCacheProvider, TranscriptCacheMount } from "./transcript-cache";

afterEach(cleanup);

it("evicts a deleted/rebound hidden entry immediately without touching the visible lease", () => {
  const view = render(<Harness active="a" />);
  const a = view.getByTestId("a");
  const host = a.closest('[data-transcript-cache-host]')!;
  view.rerender(<Harness active="b" />);
  const b = view.getByTestId("b");
  view.rerender(<Harness active="b" validKeys={["b"]} />);
  expect(view.queryByTestId("a")).toBeNull();
  expect(host.childNodes).toHaveLength(0);
  expect(host.isConnected).toBe(false);
  expect(view.getByTestId("b")).toBe(b);
});

it("survives strict effect/ref cycles and completely tears down portal children", () => {
  measurements.length = 0;
  const view = render(<StrictMode><Harness active="a" /></StrictMode>);
  const a = view.getByTestId("a");
  const host = a.closest('[data-transcript-cache-host]')!;
  view.rerender(<StrictMode><Harness active="b" /></StrictMode>);
  view.rerender(<StrictMode><Harness active="a" /></StrictMode>);
  expect(view.getByTestId("a")).toBe(a);
  expect(measurements.every(Boolean)).toBe(true);
  expect(effects).toEqual(new Set(["a"]));
  view.unmount();
  expect(effects.size).toBe(0);
  expect(layouts.size).toBe(0);
  expect(host.childNodes).toHaveLength(0);
  expect(host.isConnected).toBe(false);
});

it("disconnects hidden external stores and keyboard listeners, reading the latest snapshot on reveal", () => {
  let value = "before";
  const subscribers = new Set<() => void>();
  const renders: string[] = [];
  const keys: string[] = [];
  const subscribe = (notify: () => void) => { subscribers.add(notify); return () => { subscribers.delete(notify); }; };
  function Subscriber({ id }: { id: string }) {
    const snapshot = useSyncExternalStore(subscribe, () => value);
    renders.push(id);
    useEffect(() => {
      const key = () => keys.push(id);
      document.addEventListener("keydown", key);
      return () => document.removeEventListener("keydown", key);
    }, [id]);
    return <button>{id}:{snapshot}</button>;
  }
  function app(id: string) { return <TranscriptCacheProvider activeKey={id} validKeys={["a", "b"]}>
    <TranscriptCacheMount key={id} cacheKey={id}><Subscriber id={id} /></TranscriptCacheMount>
  </TranscriptCacheProvider>; }
  const view = render(app("a"));
  const original = view.getByText("a:before");
  view.rerender(app("b"));
  expect(subscribers.size).toBe(1);
  const aRenders = renders.filter((id) => id === "a").length;
  value = "after";
  view.rerender(app("b"));
  expect(renders.filter((id) => id === "a")).toHaveLength(aRenders);
  fireEvent.keyDown(document, { key: "Home" });
  expect(keys).toEqual(["b"]);
  view.rerender(app("a"));
  expect(view.getByText("a:after")).toBe(original);
  expect(subscribers.size).toBe(1);
  view.unmount();
  expect(subscribers.size).toBe(0);
});

it("parks hidden content inert and never restores its old focus on reveal", () => {
  const view = render(<><button data-testid="outside">Switch</button><Harness active="a" /></>);
  const original = view.getByTestId("a");
  original.focus();
  view.rerender(<><button data-testid="outside">Switch</button><Harness active="b" /></>);
  const parking = original.closest('[data-transcript-cache-parking]')!;
  expect(parking.hasAttribute("hidden")).toBe(true);
  expect(parking.hasAttribute("inert")).toBe(true);
  expect(parking.getAttribute("aria-hidden")).toBe("true");
  expect(document.activeElement).not.toBe(original);
  const outside = view.getByTestId("outside");
  outside.focus();
  view.rerender(<><button data-testid="outside">Switch</button><Harness active="a" /></>);
  expect(document.activeElement).toBe(outside);
  expect(view.getByTestId("a")).toBe(original);
});

it("restores browser-lost scroll offsets before reconnecting layout effects and before paint", () => {
  const observed: number[] = [];
  function Scroller() {
    const ref = useRef<HTMLDivElement>(null);
    useLayoutEffect(() => { observed.push(ref.current!.scrollTop); }, []);
    return <div ref={ref} data-slot="transcript-list" data-testid="scroller" />;
  }
  function app(active: string) { return <TranscriptCacheProvider activeKey={active} validKeys={["a", "b"]}>
    <TranscriptCacheMount key={active} cacheKey={active}><Scroller /></TranscriptCacheMount>
  </TranscriptCacheProvider>; }
  const append = HTMLElement.prototype.appendChild;
  const spy = vi.spyOn(HTMLElement.prototype, "appendChild").mockImplementation(function<T extends Node>(this: HTMLElement, node: T): T {
    const result = append.call(this, node) as T;
    if (node instanceof HTMLElement && node.hasAttribute("data-transcript-cache-host")) {
      node.querySelectorAll<HTMLElement>('[data-slot="transcript-list"]').forEach((scroll) => { scroll.scrollTop = 0; });
    }
    return result;
  });
  try {
    const view = render(app("a"));
    const scroll = view.getByTestId("scroller");
    scroll.scrollTop = 400;
    view.rerender(app("b"));
    view.rerender(app("a"));
    expect(observed[observed.length - 1]).toBe(400);
    expect(scroll.scrollTop).toBe(400);
  } finally { spy.mockRestore(); }
});

it("never reparents an already-active host for stream or metadata updates", () => {
  const view = render(<Harness active="a" text="first" />);
  const row = view.getByTestId("a");
  const slot = row.closest('[data-transcript-cache-slot]')!;
  const append = vi.spyOn(slot, "appendChild");
  row.focus();
  view.rerender(<Harness active="a" text="streamed" />);
  expect(append).not.toHaveBeenCalled();
  expect(document.activeElement).toBe(row);
  expect(row.textContent).toBe("streamed");
});

it("lets a fresh navigation intent override the restored scroll position", () => {
  function Scroller({ jump }: { jump?: number }) {
    const ref = useRef<HTMLDivElement>(null);
    useLayoutEffect(() => { if (jump !== undefined) ref.current!.scrollTop = jump; }, [jump]);
    return <div ref={ref} data-testid="scroller" />;
  }
  function app(active: string, jump?: number) { return <TranscriptCacheProvider activeKey={active} validKeys={["a", "b"]}>
    <TranscriptCacheMount key={active} cacheKey={active}><Scroller jump={jump} /></TranscriptCacheMount>
  </TranscriptCacheProvider>; }
  const view = render(app("a"));
  const scroll = view.getByTestId("scroller");
  scroll.scrollTop = 400;
  view.rerender(app("b"));
  view.rerender(app("a", 900));
  expect(scroll.scrollTop).toBe(900);
  // No stale saved position is reapplied on subsequent active updates.
  view.rerender(app("a", 1200));
  expect(scroll.scrollTop).toBe(1200);
});

const effects = new Set<string>();
const layouts = new Set<string>();
const measurements: boolean[] = [];
function Probe({ id, text, onClick }: { id: string; text: string; onClick?: () => void }) {
  const ref = useRef<HTMLButtonElement>(null);
  useLayoutEffect(() => {
    layouts.add(id);
    measurements.push(!!ref.current?.closest('[data-transcript-cache-slot]'));
    return () => { layouts.delete(id); };
  }, [id]);
  useEffect(() => {
    effects.add(id);
    return () => { effects.delete(id); };
  }, [id]);
  return <button ref={ref} data-testid={id} onClick={onClick}>{text}</button>;
}
function Harness({ active, text = active, onClick, validKeys = ["a", "b", "c", "d", "e"] }: { active: string; text?: string; onClick?: () => void; validKeys?: string[] }) {
  return <TranscriptCacheProvider activeKey={active} validKeys={validKeys}>
    <section key={active}>
      <input data-testid="composer" />
      <TranscriptCacheMount cacheKey={active}>
        <Probe id={active} text={text} onClick={onClick} />
      </TranscriptCacheMount>
    </section>
  </TranscriptCacheProvider>;
}

it("retains only transcript DOM across pane remounts and measures in the active slot", () => {
  measurements.length = 0;
  const oldHandler = vi.fn();
  const newHandler = vi.fn();
  const view = render(<Harness active="a" onClick={oldHandler} />);
  const original = view.getByTestId("a");
  const composer = view.getByTestId("composer");
  view.rerender(<Harness active="b" />);
  expect(effects).toEqual(new Set(["b"]));
  expect(layouts).toEqual(new Set(["b"]));
  expect(original.closest('[data-transcript-cache-parking]')).not.toBeNull();
  view.rerender(<Harness active="a" text="latest answer" onClick={newHandler} />);
  expect(view.getByTestId("a")).toBe(original);
  expect(view.getByTestId("composer")).not.toBe(composer);
  expect(original.textContent).toBe("latest answer");
  fireEvent.click(original);
  expect(newHandler).toHaveBeenCalledOnce();
  expect(oldHandler).not.toHaveBeenCalled();
  expect(measurements.every(Boolean)).toBe(true);
  expect(effects).toEqual(new Set(["a"]));
});

it("bounds ownership to four transcripts and evicts the least recently used inactive host", () => {
  const view = render(<Harness active="a" />);
  const a = view.getByTestId("a");
  for (const active of ["b", "c", "d", "a", "e"]) view.rerender(<Harness active={active} />);
  expect(view.container.querySelectorAll('[data-transcript-cache-host]')).toHaveLength(4);
  expect(view.queryByTestId("b")).toBeNull();
  expect(view.getByTestId("a")).toBe(a);
  const e = view.getByTestId("e");
  expect(e.closest('[data-transcript-cache-slot]')).not.toBeNull();
  view.rerender(<Harness active="a" />);
  expect(view.getByTestId("a")).toBe(a);
  expect(effects).toEqual(new Set(["a"]));
});
