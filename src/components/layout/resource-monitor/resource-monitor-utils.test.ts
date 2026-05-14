/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, expect, it } from "vitest";
import { formatCpu, formatMemory, formatPercent } from "./utils/formatters";
import {
  getTrackedHostMemorySeverity,
  getUsageSeverity,
} from "./utils/resource-severity";

const MB = 1024 * 1024;
const GB = MB * 1024;

describe("formatters", () => {
  it("formatMemory scales by unit", () => {
    expect(formatMemory(512 * 1024)).toBe("512 KB");
    expect(formatMemory(84 * MB)).toBe("84.0 MB");
    expect(formatMemory(2.5 * GB)).toBe("2.50 GB");
  });

  it("formatMemory clamps junk input", () => {
    expect(formatMemory(0)).toBe("0 KB");
    expect(formatMemory(-1)).toBe("0 KB");
    expect(formatMemory(Number.NaN)).toBe("0 KB");
  });

  it("formatCpu keeps one decimal and can exceed 100", () => {
    expect(formatCpu(12.34)).toBe("12.3%");
    expect(formatCpu(240)).toBe("240.0%");
    expect(formatCpu(Number.NaN)).toBe("0.0%");
  });

  it("formatPercent rounds to a whole number", () => {
    expect(formatPercent(36.7)).toBe("37%");
    expect(formatPercent(-5)).toBe("0%");
  });
});

describe("getUsageSeverity", () => {
  const calm = { cpu: 0, memory: 0 };

  it("flags high on absolute CPU or memory", () => {
    expect(getUsageSeverity({ cpu: 130, memory: 0 }, calm)).toBe("high");
    expect(getUsageSeverity({ cpu: 0, memory: 4 * GB }, calm)).toBe("high");
  });

  it("flags elevated on moderate absolute usage", () => {
    expect(getUsageSeverity({ cpu: 80, memory: 0 }, calm)).toBe("elevated");
    expect(getUsageSeverity({ cpu: 0, memory: 2 * GB }, calm)).toBe("elevated");
  });

  it("stays normal when small and the parent is not under pressure", () => {
    expect(
      getUsageSeverity({ cpu: 5, memory: 100 * MB }, { cpu: 10, memory: 1 * GB }),
    ).toBe("normal");
  });

  it("flags share-based severity when the parent is under pressure", () => {
    // Parent is busy (80% CPU) and this row owns most of it.
    const severity = getUsageSeverity(
      { cpu: 50, memory: 0 },
      { cpu: 80, memory: 0 },
    );
    expect(severity).toBe("high");
  });

  it("ignores share when includeShare is false", () => {
    expect(
      getUsageSeverity(
        { cpu: 50, memory: 0 },
        { cpu: 80, memory: 0 },
        { includeShare: false },
      ),
    ).toBe("normal");
  });
});

describe("getTrackedHostMemorySeverity", () => {
  it("maps RAM share percent to severity bands", () => {
    expect(getTrackedHostMemorySeverity(10)).toBe("normal");
    expect(getTrackedHostMemorySeverity(25)).toBe("elevated");
    expect(getTrackedHostMemorySeverity(40)).toBe("high");
  });
});
