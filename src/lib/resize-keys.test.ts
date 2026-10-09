import { describe, expect, it } from "vitest";

import { resizeKeyAction } from "./resize-keys";

describe("resizeKeyAction", () => {
  it("moves a vertical separator with left and right arrows", () => {
    expect(resizeKeyAction({ key: "ArrowRight", shiftKey: false }, "vertical")).toEqual({ kind: "move", px: 16 });
    expect(resizeKeyAction({ key: "ArrowLeft", shiftKey: false }, "vertical")).toEqual({ kind: "move", px: -16 });
    expect(resizeKeyAction({ key: "ArrowDown", shiftKey: false }, "vertical")).toBeNull();
  });

  it("moves a horizontal separator with up and down arrows", () => {
    expect(resizeKeyAction({ key: "ArrowDown", shiftKey: false }, "horizontal")).toEqual({ kind: "move", px: 16 });
    expect(resizeKeyAction({ key: "ArrowUp", shiftKey: false }, "horizontal")).toEqual({ kind: "move", px: -16 });
    expect(resizeKeyAction({ key: "ArrowRight", shiftKey: false }, "horizontal")).toBeNull();
  });

  it("takes a larger step with Shift", () => {
    expect(resizeKeyAction({ key: "ArrowRight", shiftKey: true }, "vertical")).toEqual({ kind: "move", px: 64 });
  });

  it("jumps to the bounds with Home and End", () => {
    expect(resizeKeyAction({ key: "Home", shiftKey: false }, "vertical")).toEqual({ kind: "min" });
    expect(resizeKeyAction({ key: "End", shiftKey: false }, "horizontal")).toEqual({ kind: "max" });
  });

  it("ignores every other key", () => {
    expect(resizeKeyAction({ key: "Enter", shiftKey: false }, "vertical")).toBeNull();
  });
});
