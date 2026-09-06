import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { observeTranscriptReady } from "./transcript-ready";

function rect(top: number, height: number): DOMRect {
  return { top, bottom: top + height, left: 0, right: 300, width: 300, height, x: 0, y: top, toJSON: () => ({}) };
}

let root: HTMLDivElement;
let viewport: HTMLDivElement;
let rows: HTMLDivElement;
let row: HTMLDivElement;
beforeEach(() => {
  vi.useFakeTimers();
  root = document.createElement("div");
  viewport = document.createElement("div");
  viewport.dataset.slot = "transcript-list";
  rows = document.createElement("div");
  row = document.createElement("div");
  row.dataset.index = "42";
  row.textContent = "The last answer";
  rows.append(row);
  viewport.append(rows);
  root.append(viewport);
  document.body.append(root);
  viewport.getBoundingClientRect = () => rect(20, 300);
  row.getBoundingClientRect = () => rect(100, 40);
});
afterEach(() => {
  root.remove();
  vi.useRealTimers();
});

describe("transcript visual readiness", () => {
  it("waits for virtualizer opacity and a paint opportunity, not mounted text", () => {
    rows.style.opacity = "0";
    const ready = vi.fn();
    observeTranscriptReady(root, ready);
    vi.advanceTimersByTime(100);
    expect(ready).not.toHaveBeenCalled();
    rows.style.opacity = "1";
    vi.advanceTimersToNextFrame();
    expect(ready).not.toHaveBeenCalled();
    vi.advanceTimersToNextFrame();
    expect(ready).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(200);
    expect(ready).toHaveBeenCalledTimes(1);
  });

  it("waits until text intersects the transcript viewport", () => {
    row.getBoundingClientRect = () => rect(500, 40);
    const ready = vi.fn();
    observeTranscriptReady(root, ready);
    vi.advanceTimersByTime(100);
    expect(ready).not.toHaveBeenCalled();
    row.getBoundingClientRect = () => rect(100, 40);
    vi.advanceTimersByTime(40);
    expect(ready).toHaveBeenCalledOnce();
  });

  it("does not report a pane switched away before its next paint", () => {
    const ready = vi.fn();
    const cancel = observeTranscriptReady(root, ready);
    vi.advanceTimersToNextFrame();
    cancel();
    vi.advanceTimersByTime(100);
    expect(ready).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("rechecks visibility after the paint opportunity", () => {
    const ready = vi.fn();
    const cancel = observeTranscriptReady(root, ready);
    vi.advanceTimersToNextFrame();
    rows.style.display = "none";
    vi.advanceTimersToNextFrame();
    expect(ready).not.toHaveBeenCalled();
    cancel();
  });

  it("stops without inventing readiness if no content becomes visible", () => {
    row.textContent = "";
    const ready = vi.fn();
    observeTranscriptReady(root, ready);
    vi.advanceTimersByTime(11_000);
    expect(ready).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("ignores a detached pane", () => {
    const ready = vi.fn();
    observeTranscriptReady(root, ready);
    root.remove();
    vi.advanceTimersByTime(40);
    expect(ready).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });
});
