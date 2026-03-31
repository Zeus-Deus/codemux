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

  it("registers reload-blocking shortcuts", () => {
    const blockReload = getRegistryEntry("blockReload");
    expect(blockReload).toBeDefined();
    expect(blockReload!.defaultKeys).toBe("Ctrl+R");

    const blockHardReload = getRegistryEntry("blockHardReload");
    expect(blockHardReload).toBeDefined();
    expect(blockHardReload!.defaultKeys).toBe("Ctrl+Shift+R");

    const blockF5 = getRegistryEntry("blockF5Reload");
    expect(blockF5).toBeDefined();
    expect(blockF5!.defaultKeys).toBe("F5");
  });
});
