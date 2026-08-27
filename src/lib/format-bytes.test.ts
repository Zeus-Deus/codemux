import { describe, expect, it } from "vitest";

import { formatBytes } from "./format-bytes";

const KB = 1024;
const MB = KB * 1024;
const GB = MB * 1024;

describe("formatBytes", () => {
  it("widens precision with the unit", () => {
    expect(formatBytes(900)).toBe("900 B");
    expect(formatBytes(512 * KB)).toBe("512 KB");
    expect(formatBytes(84 * MB)).toBe("84.0 MB");
    expect(formatBytes(2.5 * GB)).toBe("2.50 GB");
    expect(formatBytes(1.5 * GB * 1024)).toBe("1.50 TB");
  });

  it("clamps junk input", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(-1)).toBe("0 B");
    expect(formatBytes(Number.NaN)).toBe("0 B");
  });
});
