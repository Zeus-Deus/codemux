import { describe, expect, it } from "vitest";
import { isInterruptKey } from "./composer-interrupt";

const key = (init: KeyboardEventInit) => new KeyboardEvent("keydown", init);

describe("isInterruptKey", () => {
  it("matches the bound key, and Ctrl+C in an empty composer", () => {
    expect(isInterruptKey(key({ key: "Escape" }), "Escape", "draft")).toBe(true);
    expect(isInterruptKey(key({ key: "c", ctrlKey: true }), "Escape", "")).toBe(true);
    expect(isInterruptKey(key({ key: "c", ctrlKey: true }), "Escape", "draft")).toBe(false);
  });

  it("turns both off when the stop binding is removed in Settings", () => {
    expect(isInterruptKey(key({ key: "Escape" }), "", "")).toBe(false);
    expect(isInterruptKey(key({ key: "c", ctrlKey: true }), "", "")).toBe(false);
  });
});
