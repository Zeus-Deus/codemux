import { describe, it, expect } from "vitest";
import { resolveKeybinds } from "./use-resolved-keybinds";

describe("resolveKeybinds", () => {
  it("uses defaults when no overrides", () => {
    const result = resolveKeybinds({});
    expect(result.getKeysForAction("commandPalette")).toBe("Ctrl+K");
    expect(result.getKeysForAction("splitPaneRight")).toBe("Ctrl+Shift+D");
  });

  it("override replaces default", () => {
    const result = resolveKeybinds({ commandPalette: "Ctrl+J" });
    expect(result.getKeysForAction("commandPalette")).toBe("Ctrl+J");
  });

  it("empty string override unbinds action", () => {
    const result = resolveKeybinds({ commandPalette: "" });
    expect(result.getKeysForAction("commandPalette")).toBe("");
  });

  it("unknown IDs in overrides are ignored", () => {
    const result = resolveKeybinds({ unknownAction: "Ctrl+X" });
    // Should not throw and should not create a new entry
    expect(result.keybindMap.has("unknownAction")).toBe(false);
  });

  it("detects conflicts when two actions share a combo", () => {
    const result = resolveKeybinds({
      commandPalette: "Ctrl+P", // conflicts with fileSearch default
    });
    expect(result.conflicts.length).toBeGreaterThan(0);
    const conflict = result.conflicts.find((c) => c.combo === "Ctrl+P");
    expect(conflict).toBeDefined();
    expect(conflict!.ids).toContain("commandPalette");
    expect(conflict!.ids).toContain("fileSearch");
  });

  it("no conflicts with default bindings", () => {
    const result = resolveKeybinds({});
    expect(result.conflicts).toHaveLength(0);
  });

  it("reverse map finds action for combo", () => {
    const result = resolveKeybinds({});
    expect(result.getActionForKeys("Ctrl+K")).toBe("commandPalette");
  });

  it("reverse map returns null for unbound combo", () => {
    const result = resolveKeybinds({});
    expect(result.getActionForKeys("Ctrl+Alt+Shift+Z")).toBeNull();
  });

  it("isCustom flag is set for overrides", () => {
    const result = resolveKeybinds({ commandPalette: "Ctrl+J" });
    const entry = result.keybindMap.get("commandPalette");
    expect(entry?.isCustom).toBe(true);
  });

  it("isCustom flag is false when override matches default", () => {
    const result = resolveKeybinds({ commandPalette: "Ctrl+K" });
    const entry = result.keybindMap.get("commandPalette");
    expect(entry?.isCustom).toBe(false);
  });

  it("isCustom flag is false with no overrides", () => {
    const result = resolveKeybinds({});
    const entry = result.keybindMap.get("commandPalette");
    expect(entry?.isCustom).toBe(false);
  });

  describe("conflict override → reset flow", () => {
    // Simulates: user rebinds fileSearch to Ctrl+K (commandPalette's default),
    // confirms conflict → commandPalette gets unbound → then resets commandPalette.
    const CONFLICT_OVERRIDES = { fileSearch: "Ctrl+K", commandPalette: "" };

    it("unbound-via-conflict action has isCustom=true", () => {
      const result = resolveKeybinds(CONFLICT_OVERRIDES);
      const entry = result.keybindMap.get("commandPalette")!;
      expect(entry.activeKeys).toBe("");
      expect(entry.isCustom).toBe(true);
    });

    it("overriding action has correct combo and isCustom=true", () => {
      const result = resolveKeybinds(CONFLICT_OVERRIDES);
      const entry = result.keybindMap.get("fileSearch")!;
      expect(entry.activeKeys).toBe("Ctrl+K");
      expect(entry.isCustom).toBe(true);
    });

    it("resetting the unbound action restores its default", () => {
      // Simulate removeOverride("commandPalette"): delete the key
      const afterReset = { fileSearch: "Ctrl+K" };
      const result = resolveKeybinds(afterReset);
      const entry = result.keybindMap.get("commandPalette")!;
      expect(entry.activeKeys).toBe("Ctrl+K");
      expect(entry.isCustom).toBe(false);
    });

    it("Reset All clears everything including conflict-unbound entries", () => {
      const result = resolveKeybinds({});
      const cp = result.keybindMap.get("commandPalette")!;
      const fs = result.keybindMap.get("fileSearch")!;
      expect(cp.activeKeys).toBe("Ctrl+K");
      expect(cp.isCustom).toBe(false);
      expect(fs.activeKeys).toBe("Ctrl+P");
      expect(fs.isCustom).toBe(false);
    });
  });
});
