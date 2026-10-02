import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { usePopupArrowNavigation } from "./use-popup-arrow-navigation";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const press = (key: "ArrowUp" | "ArrowDown", repeat: boolean, ageMs = 0) => ({
  key,
  repeat,
  timeStamp: performance.now() - ageMs,
});

function setup() {
  const frames = new Map<number, FrameRequestCallback>();
  let nextFrame = 0;
  vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
    frames.set(++nextFrame, callback);
    return nextFrame;
  });
  vi.spyOn(window, "cancelAnimationFrame").mockImplementation((id) => frames.delete(id));
  const move = vi.fn();
  const hook = renderHook(({ enabled }) => usePopupArrowNavigation(enabled, move), {
    initialProps: { enabled: true },
  });
  const tick = () => act(() => {
    const callbacks = [...frames.values()];
    frames.clear();
    callbacks.forEach((callback) => callback(performance.now()));
  });
  return { ...hook, move, frames, tick };
}

describe("popup arrow navigation", () => {
  it.each(["ArrowDown", "ArrowUp"] as const)("coalesces a held %s and stops immediately on release", (key) => {
    const { result, move, frames, tick } = setup();
    const direction = key === "ArrowDown" ? 1 : -1;
    act(() => result.current.navigate(press(key, false)));
    expect(move).toHaveBeenLastCalledWith(direction);
    for (let i = 0; i < 50; i++) act(() => result.current.navigate(press(key, true)));
    expect(frames.size).toBe(1);
    tick();
    expect(move).toHaveBeenCalledTimes(2);
    act(() => result.current.navigate(press(key, true)));
    const queuedFrame = [...frames.values()][0];
    act(() => window.dispatchEvent(new KeyboardEvent("keyup", { key })));
    expect(frames.size).toBe(0);
    // Even a late native repeat or an already dequeued callback cannot restart it.
    act(() => {
      queuedFrame(performance.now());
      result.current.navigate(press(key, true));
    });
    tick();
    expect(move).toHaveBeenCalledTimes(2);
    act(() => result.current.navigate(press(key, false)));
    expect(move).toHaveBeenCalledTimes(3);
  });

  it("drops repeats that queued up behind slow frames", () => {
    const { result, move, frames, tick } = setup();
    act(() => result.current.navigate(press("ArrowDown", false)));
    // WebKitGTK hands over backed-up repeats long after they were pressed.
    for (let i = 0; i < 50; i++) act(() => result.current.navigate(press("ArrowDown", true, 500)));
    expect(frames.size).toBe(0);
    act(() => result.current.navigate(press("ArrowDown", true)));
    tick();
    expect(move).toHaveBeenCalledTimes(2);
  });

  it("cancels pending movement on focus loss", () => {
    const { result, move, frames, tick } = setup();
    act(() => {
      result.current.navigate(press("ArrowDown", false));
      result.current.navigate(press("ArrowDown", true));
      window.dispatchEvent(new Event("blur"));
    });
    expect(frames.size).toBe(0);
    tick();
    expect(move).toHaveBeenCalledTimes(1);
  });

  it("cancels pending movement when the popup closes or unmounts", () => {
    const { result, move, frames, tick, rerender, unmount } = setup();
    act(() => {
      result.current.navigate(press("ArrowDown", false));
      result.current.navigate(press("ArrowDown", true));
    });
    rerender({ enabled: false });
    expect(frames.size).toBe(0);
    tick();
    expect(move).toHaveBeenCalledTimes(1);
    rerender({ enabled: true });
    act(() => {
      result.current.navigate(press("ArrowDown", false));
      result.current.navigate(press("ArrowDown", true));
    });
    unmount();
    expect(frames.size).toBe(0);
    tick();
    expect(move).toHaveBeenCalledTimes(2);
  });
});
