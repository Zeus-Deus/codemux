import { describe, it, expect } from "vitest";

import { updateAppShortcuts, isAppShortcut } from "./app-shortcuts";
import { matchesKeyCombo } from "./keybind-utils";

function keyEvent(partial: Partial<KeyboardEvent>): KeyboardEvent {
  return {
    key: partial.key ?? "",
    ctrlKey: partial.ctrlKey ?? false,
    shiftKey: partial.shiftKey ?? false,
    altKey: partial.altKey ?? false,
  } as KeyboardEvent;
}

describe("isAppShortcut", () => {
  it("intercepts a Space-bound app shortcut (event.key is ' ')", () => {
    // Fresh object ref so the reference-equality guard doesn't short-circuit.
    updateAppShortcuts({ commandPalette: "Space" });
    const ev = keyEvent({ key: " " });
    // The canonical matcher agrees this event IS the "Space" binding…
    expect(matchesKeyCombo(ev, "Space")).toBe(true);
    // …so the app-shortcut matcher must agree too.
    expect(isAppShortcut(ev)).toBe(true);
  });

  it("still matches a normal Ctrl combo", () => {
    updateAppShortcuts({ commandPalette: "Ctrl+K" });
    expect(isAppShortcut(keyEvent({ key: "k", ctrlKey: true }))).toBe(true);
    // Modifier mismatch must not match.
    expect(isAppShortcut(keyEvent({ key: "k" }))).toBe(false);
  });
});
