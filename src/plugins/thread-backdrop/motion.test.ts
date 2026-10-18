import { afterEach, describe, expect, it, vi } from "vitest";
import type { Host } from "@codemux/sdk";
import { watchReducedMotion } from "./Backdrop";

/** A host whose reduce-motion setting can be flipped, and a fake OS query. */
function setup() {
  let appSetting = false;
  const settingListeners = new Set<(v: string) => void>();
  const host = {
    reducedMotion: () => appSetting,
    settings: {
      subscribe: (key: string, fn: (v: string) => void) => {
        expect(key).toBe("appearance.reduce_motion");
        settingListeners.add(fn);
        return () => settingListeners.delete(fn);
      },
    },
  } as unknown as Host;
  const query = { matches: false, listeners: new Set<() => void>() };
  vi.stubGlobal("matchMedia", () => ({
    get matches() {
      return query.matches;
    },
    addEventListener: (_: string, fn: () => void) => query.listeners.add(fn),
    removeEventListener: (_: string, fn: () => void) => query.listeners.delete(fn),
  }));
  return {
    host,
    setApp(v: boolean) {
      appSetting = v;
      for (const fn of settingListeners) fn(String(v));
    },
    setOs(v: boolean) {
      query.matches = v;
      for (const fn of query.listeners) fn();
    },
    listenerCount: () => settingListeners.size + query.listeners.size,
  };
}

afterEach(() => vi.unstubAllGlobals());

describe("watchReducedMotion", () => {
  it("reports changes from the app setting and from the OS, once each", () => {
    const t = setup();
    const seen: boolean[] = [];
    const off = watchReducedMotion(t.host, (r) => seen.push(r));
    t.setApp(true);
    t.setOs(true); // still reduced: no repeat
    t.setApp(false); // OS still reduces
    t.setOs(false);
    expect(seen).toEqual([true, false]);
    off();
    expect(t.listenerCount()).toBe(0);
    t.setApp(true);
    expect(seen).toEqual([true, false]);
  });
});
