import { describe, expect, it, vi } from "vitest";

import {
  clearTitlebarContentUnder,
  getTitlebarContentUnder,
  publishTitlebarContentUnder,
  subscribeTitlebarContentUnder,
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
