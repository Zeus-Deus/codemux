import { describe, expect, it, vi } from "vitest";
import {
  addonComposer,
  appendAddonText,
  composerForWorkspace,
  registerAddonComposer,
} from "./composer-registry";
describe("add-on draft targeting", () => {
  it("appends literal text without replacing user input and bounds UTF-8 bytes", () => {
    expect(appendAddonText("unsent draft", "brief")).toBe(
      "unsent draft\nbrief",
    );
    expect(appendAddonText("unsent\n", "brief")).toBe("unsent\nbrief");
    expect(appendAddonText("unsent", "")).toBe("unsent");
    expect(() => appendAddonText("unsent", "é".repeat(16385))).toThrow();
  });
  it("never guesses between live composers and rejects disposed or mismatched targets", () => {
    const first = { workspaceId: "w", append: vi.fn(() => 1) };
    const remove = registerAddonComposer("one", first);
    expect(composerForWorkspace("w")).toBe("one");
    const removeSecond = registerAddonComposer("two", {
      workspaceId: "w",
      append: vi.fn(() => 1),
    });
    expect(composerForWorkspace("w")).toBeNull();
    expect(() => addonComposer("one", "other")).toThrow();
    remove();
    expect(() => addonComposer("one", "w")).toThrow();
    expect(first.append).not.toHaveBeenCalled();
    removeSecond();
  });
  it("old cleanup cannot unregister a replacement instance", () => {
    const old = registerAddonComposer("replacement", {
      workspaceId: "w",
      append: () => 1,
    });
    const current = { workspaceId: "w", append: () => 2 };
    const cleanup = registerAddonComposer("replacement", current);
    old();
    expect(addonComposer("replacement", "w")).toBe(current);
    cleanup();
  });
});
