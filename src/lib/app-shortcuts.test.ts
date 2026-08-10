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

  it("leaves `non-terminal` and `terminal` bindings to the pty", () => {
    updateAppShortcuts({ commandPalette: "Ctrl+K" });
    // Rename-workspace (F2) must reach htop / mc / nano untouched…
    expect(isAppShortcut(keyEvent({ key: "F2" }))).toBe(false);
    // …as must the shortcuts xterm handles itself.
    expect(
      isAppShortcut(keyEvent({ key: "C", ctrlKey: true, shiftKey: true })),
    ).toBe(false);
    // A rebind of the same action stays out of the intercept list too.
    updateAppShortcuts({ renameWorkspace: "F6" });
    expect(isAppShortcut(keyEvent({ key: "F6" }))).toBe(false);
    // Sanity: an "always" binding on a bare function key still intercepts.
    updateAppShortcuts({ commandPalette: "F6" });
    expect(isAppShortcut(keyEvent({ key: "F6" }))).toBe(true);
  });
});
