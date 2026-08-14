import { afterEach, describe, expect, it, vi } from "vitest";

import { discoverInstalledFontFamilies, normalizeDiscoveredFamilies } from "./font-discovery";

describe("font discovery", () => {
  it("deduplicates case-insensitively, sorts, and always includes bundled and generic fonts", () => {
    const families = normalizeDiscoveredFamilies([
      "Fira Code",
      "fira code",
      "  Aptos  ",
      "",
    ]);

    expect(families.filter((family) => family.toLowerCase() === "fira code")).toHaveLength(1);
    expect(families).toContain("Aptos");
    expect(families).toContain("DM Sans Variable");
    expect(families).toContain("JetBrains Mono Variable");
    expect(families).toContain("system-ui");
    expect(families).toEqual([...families].sort((a, b) => a.localeCompare(b)));
  });

  describe("discoverInstalledFontFamilies", () => {
    afterEach(() => {
      vi.useRealTimers();
      Reflect.deleteProperty(window, "queryLocalFonts");
    });

    it("retries after a denied query but caches a real enumeration", async () => {
      vi.useFakeTimers();
      const queryLocalFonts = vi
        .fn()
        .mockRejectedValueOnce(new Error("denied"))
        .mockResolvedValue([{ family: "Berkeley Mono" }]);
      Object.defineProperty(window, "queryLocalFonts", {
        configurable: true,
        writable: true,
        value: queryLocalFonts,
      });

      // The denied fallback is shared while it is fresh, then re-asked.
      expect(await discoverInstalledFontFamilies()).not.toContain("Berkeley Mono");
      expect(await discoverInstalledFontFamilies()).not.toContain("Berkeley Mono");
      expect(queryLocalFonts).toHaveBeenCalledTimes(1);

      vi.advanceTimersByTime(60_000);
      expect(await discoverInstalledFontFamilies()).toContain("Berkeley Mono");
      expect(await discoverInstalledFontFamilies()).toContain("Berkeley Mono");
      expect(queryLocalFonts).toHaveBeenCalledTimes(2);
    });
  });
});
