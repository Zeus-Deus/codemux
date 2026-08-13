import { describe, expect, it } from "vitest";

import { normalizeDiscoveredFamilies } from "./font-discovery";

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
});
