import { describe, expect, it, vi } from "vitest";

import {
  clearTitlebarContentUnder,
  getTitlebarContentUnder,
  getTitlebarTranscriptElements,
  getTitlebarTranscriptVersion,
  publishTitlebarContentUnder,
  registerTitlebarTranscript,
  subscribeTitlebarContentUnder,
  subscribeTitlebarTranscripts,
} from "./titlebar-content-under";

describe("titlebar content-under state", () => {
  it("stays quiet at scroll-top and activates only for content underneath", () => {
    const source = Symbol("pane");
    const listener = vi.fn();
    const unsubscribe = subscribeTitlebarContentUnder(listener);

    publishTitlebarContentUnder("ws-state", source, false);
    expect(getTitlebarContentUnder("ws-state")).toBe(false);
    expect(listener).not.toHaveBeenCalled();

    publishTitlebarContentUnder("ws-state", source, true);
    expect(getTitlebarContentUnder("ws-state")).toBe(true);
    expect(listener).toHaveBeenCalledTimes(1);

    clearTitlebarContentUnder("ws-state", source);
    expect(getTitlebarContentUnder("ws-state")).toBe(false);
    expect(listener).toHaveBeenCalledTimes(2);
    unsubscribe();
  });

  it("aggregates split chat panes without one pane clearing another", () => {
    const first = Symbol("first-pane");
    const second = Symbol("second-pane");

    publishTitlebarContentUnder("ws-split", first, true);
    publishTitlebarContentUnder("ws-split", second, false);
    clearTitlebarContentUnder("ws-split", second);
    expect(getTitlebarContentUnder("ws-split")).toBe(true);

    clearTitlebarContentUnder("ws-split", first);
    expect(getTitlebarContentUnder("ws-split")).toBe(false);
  });
});

describe("titlebar transcript registry", () => {
  it("bumps its version as viewports mount and unmount so the titlebar re-measures", () => {
    const listener = vi.fn();
    const unsubscribe = subscribeTitlebarTranscripts(listener);
    const before = getTitlebarTranscriptVersion();

    const first = document.createElement("div");
    const unregisterFirst = registerTitlebarTranscript(first);
    expect(getTitlebarTranscriptElements()).toContain(first);
    expect(getTitlebarTranscriptVersion()).toBe(before + 1);
    expect(listener).toHaveBeenCalledTimes(1);

    // Switching tabs unmounts the old list and mounts a fresh node — both
    // halves must be observable, otherwise the titlebar keeps measuring a
    // detached element forever.
    unregisterFirst();
    const second = document.createElement("div");
    const unregisterSecond = registerTitlebarTranscript(second);
    expect(getTitlebarTranscriptElements()).toEqual([second]);
    expect(listener).toHaveBeenCalledTimes(3);

    // Re-registering the same node is a no-op, so a re-render can't spin
    // the version (and the effect it keys) forever.
    registerTitlebarTranscript(second);
    expect(listener).toHaveBeenCalledTimes(3);

    unregisterSecond();
    expect(getTitlebarTranscriptElements()).toEqual([]);
    unsubscribe();
  });

  it("tolerates a double unregister from a re-run cleanup", () => {
    const element = document.createElement("div");
    const unregister = registerTitlebarTranscript(element);
    unregister();
    const version = getTitlebarTranscriptVersion();
    unregister();
    expect(getTitlebarTranscriptVersion()).toBe(version);
  });
});
