import { describe, it, expect } from "vitest";
import { KEYBIND_REGISTRY, getRegistryEntry, KEYBIND_CATEGORIES } from "./keybind-registry";
import { parseKeyCombo } from "./keybind-utils";

describe("keybind-registry", () => {
  it("has no duplicate IDs", () => {
    const ids = KEYBIND_REGISTRY.map((e) => e.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("has no duplicate default key combos (excluding same-category tab switches)", () => {
    const combos = KEYBIND_REGISTRY.map((e) => e.defaultKeys);
    expect(new Set(combos).size).toBe(combos.length);
  });

  it("every entry has a non-empty label", () => {
    for (const entry of KEYBIND_REGISTRY) {
      expect(entry.label.length, `${entry.id} has empty label`).toBeGreaterThan(0);
    }
  });

  it("every entry has a valid category", () => {
    const validCategories = new Set(KEYBIND_CATEGORIES);
    for (const entry of KEYBIND_REGISTRY) {
      expect(validCategories.has(entry.category), `${entry.id} has invalid category: ${entry.category}`).toBe(true);
    }
  });

  it("every defaultKeys parses without error", () => {
    for (const entry of KEYBIND_REGISTRY) {
      const parsed = parseKeyCombo(entry.defaultKeys);
      expect(parsed.key.length, `${entry.id} defaultKeys "${entry.defaultKeys}" has empty key`).toBeGreaterThan(0);
    }
  });

  it("getRegistryEntry returns correct entry", () => {
    const entry = getRegistryEntry("commandPalette");
    expect(entry).toBeDefined();
    expect(entry!.defaultKeys).toBe("Ctrl+K");
  });

  it("getRegistryEntry returns undefined for unknown ID", () => {
    expect(getRegistryEntry("nonExistentAction")).toBeUndefined();
  });

  it("registers workspace jump shortcuts 1-9 on Alt+digit (Ctrl+digit is tabs)", () => {
    for (let n = 1; n <= 9; n++) {
      const entry = getRegistryEntry(`workspaceJump${n}`);
      expect(entry, `workspaceJump${n} missing`).toBeDefined();
      expect(entry!.defaultKeys).toBe(`Alt+${n}`);
      expect(entry!.label).toBe(`Jump to workspace ${n}`);
      expect(entry!.category).toBe("workspaces");
    }
    // The digit jumps must not collide with the terminal tab switches.
    for (let n = 1; n <= 9; n++) {
      expect(getRegistryEntry(`switchTab${n}`)!.defaultKeys).toBe(`Ctrl+${n}`);
    }
  });

  it("keeps F2 out of the terminal's way", () => {
    const entry = getRegistryEntry("renameWorkspace");
    expect(entry).toBeDefined();
    expect(entry!.defaultKeys).toBe("F2");
    // F2 is a live key in curses apps, so a focused pty must keep it. Anything
    // other than "non-terminal" either steals it from the shell ("always") or
    // stops the window handler from renaming at all ("terminal").
    expect(entry!.when).toBe("non-terminal");
  });

  it("swallows the reload keys a terminal needs", () => {
    const blockReload = getRegistryEntry("blockReload");
    expect(blockReload).toBeDefined();
    expect(blockReload!.defaultKeys).toBe("Ctrl+R");

    const blockF5 = getRegistryEntry("blockF5Reload");
    expect(blockF5).toBeDefined();
    expect(blockF5!.defaultKeys).toBe("F5");
  });

  it("documents the recovery reload on the chord the app process grabs", () => {
    const entry = getRegistryEntry("reloadInterface");
    expect(entry).toBeDefined();
    // `webview_recovery.rs` hard-codes this combo on the GTK toplevel so it
    // works with a dead renderer; the two must not drift apart.
    expect(entry!.defaultKeys).toBe("Ctrl+Shift+R");
    // Shown in Settings → Shortcuts, so it has to explain itself.
    expect(entry!.description).toBeTruthy();
    // Window-level: the terminal gives the combo up instead of sending it to
    // the pty.
    expect(entry!.when ?? "always").toBe("always");
  });
});
