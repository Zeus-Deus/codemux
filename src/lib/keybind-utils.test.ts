import { describe, it, expect } from "vitest";
import {
  normalizeKeyCombo,
  parseKeyCombo,
  matchesKeyCombo,
  isModifierOnly,
} from "./keybind-utils";

function fakeEvent(overrides: Partial<KeyboardEvent>): KeyboardEvent {
  return {
    key: "",
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    metaKey: false,
    ...overrides,
  } as KeyboardEvent;
}

describe("normalizeKeyCombo", () => {
  it("normalizes Ctrl+letter to uppercase", () => {
    expect(normalizeKeyCombo(fakeEvent({ ctrlKey: true, key: "k" }))).toBe("Ctrl+K");
  });

  it("normalizes Ctrl+Shift+letter", () => {
    expect(normalizeKeyCombo(fakeEvent({ ctrlKey: true, shiftKey: true, key: "D" }))).toBe("Ctrl+Shift+D");
  });

  it("normalizes Ctrl+digit", () => {
    expect(normalizeKeyCombo(fakeEvent({ ctrlKey: true, key: "1" }))).toBe("Ctrl+1");
  });

  it("normalizes Escape with no modifiers", () => {
    expect(normalizeKeyCombo(fakeEvent({ key: "Escape" }))).toBe("Escape");
  });

  it("normalizes Ctrl+bracket keys", () => {
    expect(normalizeKeyCombo(fakeEvent({ ctrlKey: true, key: "]" }))).toBe("Ctrl+]");
    expect(normalizeKeyCombo(fakeEvent({ ctrlKey: true, key: "[" }))).toBe("Ctrl+[");
  });

  it("normalizes Ctrl+Backspace", () => {
    expect(normalizeKeyCombo(fakeEvent({ ctrlKey: true, key: "Backspace" }))).toBe("Ctrl+Backspace");
  });

  it("normalizes Alt+Ctrl+Shift in alphabetical modifier order", () => {
    expect(
      normalizeKeyCombo(fakeEvent({ altKey: true, ctrlKey: true, shiftKey: true, key: "X" })),
    ).toBe("Alt+Ctrl+Shift+X");
  });

  it("returns empty string for modifier-only presses", () => {
    expect(normalizeKeyCombo(fakeEvent({ key: "Control", ctrlKey: true }))).toBe("");
    expect(normalizeKeyCombo(fakeEvent({ key: "Shift", shiftKey: true }))).toBe("");
    expect(normalizeKeyCombo(fakeEvent({ key: "Alt", altKey: true }))).toBe("");
  });

  it("normalizes space key", () => {
    expect(normalizeKeyCombo(fakeEvent({ ctrlKey: true, key: " " }))).toBe("Ctrl+Space");
  });
});

describe("parseKeyCombo", () => {
  it("parses simple Ctrl+key", () => {
    expect(parseKeyCombo("Ctrl+K")).toEqual({ alt: false, ctrl: true, shift: false, key: "K" });
  });

  it("parses Ctrl+Shift+key", () => {
    expect(parseKeyCombo("Ctrl+Shift+D")).toEqual({ alt: false, ctrl: true, shift: true, key: "D" });
  });

  it("parses standalone Escape", () => {
    expect(parseKeyCombo("Escape")).toEqual({ alt: false, ctrl: false, shift: false, key: "Escape" });
  });

  it("parses Alt+Ctrl+Shift+key", () => {
    expect(parseKeyCombo("Alt+Ctrl+Shift+X")).toEqual({ alt: true, ctrl: true, shift: true, key: "X" });
  });
});

describe("matchesKeyCombo", () => {
  it("matches Ctrl+K", () => {
    expect(matchesKeyCombo(fakeEvent({ ctrlKey: true, key: "k" }), "Ctrl+K")).toBe(true);
  });

  it("rejects wrong modifier", () => {
    expect(matchesKeyCombo(fakeEvent({ ctrlKey: true, shiftKey: true, key: "k" }), "Ctrl+K")).toBe(false);
  });

  it("rejects wrong key", () => {
    expect(matchesKeyCombo(fakeEvent({ ctrlKey: true, key: "j" }), "Ctrl+K")).toBe(false);
  });

  it("matches Escape", () => {
    expect(matchesKeyCombo(fakeEvent({ key: "Escape" }), "Escape")).toBe(true);
  });

  it("matches Ctrl+Backspace", () => {
    expect(matchesKeyCombo(fakeEvent({ ctrlKey: true, key: "Backspace" }), "Ctrl+Backspace")).toBe(true);
  });

  it("returns false for empty combo", () => {
    expect(matchesKeyCombo(fakeEvent({ key: "a" }), "")).toBe(false);
  });
});

describe("isModifierOnly", () => {
  it("returns true for Control", () => {
    expect(isModifierOnly(fakeEvent({ key: "Control" }))).toBe(true);
  });

  it("returns true for Shift", () => {
    expect(isModifierOnly(fakeEvent({ key: "Shift" }))).toBe(true);
  });

  it("returns false for regular key", () => {
    expect(isModifierOnly(fakeEvent({ key: "a" }))).toBe(false);
  });

  it("returns false for Escape", () => {
    expect(isModifierOnly(fakeEvent({ key: "Escape" }))).toBe(false);
  });
});
