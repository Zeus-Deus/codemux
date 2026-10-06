import { describe, expect, it } from "vitest";

import type { PlanUsageWindow } from "@/tauri/commands";
import {
  elapsedShare,
  formatResetsIn,
  limitWindowLabel,
  paceOf,
  remainingPercent,
} from "./usage-limits";
import { isKnownProvider, seriesFill, seriesLabel } from "./usage-format";
import { parseRateDraft } from "./usage-model-dialog";
import { costTypeSegments } from "./usage-share-bar";

const MIN = 60_000;
const NOW = 1_800_000_000_000;

const w = (overrides: Partial<PlanUsageWindow>): PlanUsageWindow => ({
  kind: "five_hour",
  used_pct: 50,
  resets_at_ms: NOW + 150 * MIN,
  ...overrides,
});

describe("limit windows", () => {
  it("names windows by kind, and keeps a provider's own label", () => {
    expect(limitWindowLabel(w({ kind: "five_hour" }))).toBe("5-hour");
    expect(limitWindowLabel(w({ kind: "seven_day" }))).toBe("Weekly");
    expect(limitWindowLabel(w({ kind: "seven_day_opus" }))).toBe("Weekly · Opus");
    expect(limitWindowLabel(w({ kind: "other", label: "Weekly · Fable" }))).toBe(
      "Weekly · Fable",
    );
    expect(limitWindowLabel(w({ kind: "other", label: "1d" }))).toBe("1d window");
    expect(limitWindowLabel(w({ kind: "overage", label: "overage" }))).toBe("Extra usage");
  });

  it("reports quota left, clamped", () => {
    expect(remainingPercent(w({ used_pct: 41.4 }))).toBe(59);
    expect(remainingPercent(w({ used_pct: 140 }))).toBe(0);
  });

  it("measures elapsed time from the kind's length when none is reported", () => {
    // 150 of 300 minutes left → half elapsed.
    expect(elapsedShare(w({}), NOW)).toBeCloseTo(0.5);
    expect(elapsedShare(w({ window_mins: 600 }), NOW)).toBeCloseTo(0.75);
    expect(elapsedShare(w({ kind: "overage" }), NOW)).toBeNull();
    expect(elapsedShare(w({ resets_at_ms: null }), NOW)).toBeNull();
  });

  it("calls pace against the clock with a five-point band", () => {
    expect(paceOf(w({ used_pct: 54 }), NOW)).toBe("on");
    expect(paceOf(w({ used_pct: 70 }), NOW)).toBe("ahead");
    expect(paceOf(w({ used_pct: 20 }), NOW)).toBe("under");
    expect(paceOf(w({ kind: "overage" }), NOW)).toBeNull();
  });

  it("phrases the reset as a countdown", () => {
    expect(formatResetsIn(w({}), NOW)).toBe("resets in 2h 30m");
    expect(formatResetsIn(w({ resets_at_ms: NOW + 3 * 24 * 60 * MIN + 61 * MIN }), NOW)).toBe(
      "resets in 3d 1h",
    );
    expect(formatResetsIn(w({ resets_at_ms: NOW - 1 }), NOW)).toBe("resets now");
    expect(formatResetsIn(w({ resets_at_ms: null }), NOW)).toBeNull();
  });
});

describe("price editor parsing", () => {
  it("requires input and output, defaulting blank cache rates to zero", () => {
    expect(
      parseRateDraft({ input: "1.5", output: "6", cache_read: "", cache_write: "" }),
    ).toEqual({
      ok: true,
      price: { input: 1.5, output: 6, cache_read: 0, cache_write: 0 },
    });
    expect(
      parseRateDraft({ input: "", output: "6", cache_read: "", cache_write: "" }),
    ).toEqual({ ok: false, field: "input" });
    expect(
      parseRateDraft({ input: "1", output: "-2", cache_read: "", cache_write: "" }),
    ).toEqual({ ok: false, field: "output" });
    expect(
      parseRateDraft({ input: "1", output: "2", cache_read: "abc", cache_write: "" }),
    ).toEqual({ ok: false, field: "cache_read" });
  });
});

describe("cost by type", () => {
  it("drops sub-cent unsplit cost as rounding", () => {
    const segments = costTypeSegments({
      input: 1,
      output: 2,
      cache_read: 0.5,
      cache_write: 0.25,
      unsplit: 0.004,
    });
    expect(segments.find((s) => s.label === "Unsplit")?.value).toBe(0);
    expect(segments.find((s) => s.label === "Output")?.value).toBe(2);
  });
});

describe("provider presentation", () => {
  it("treats inherited object keys as unknown providers", () => {
    expect(isKnownProvider("claude")).toBe(true);
    expect(isKnownProvider("constructor")).toBe(false);
    expect(seriesLabel("constructor")).toBe("constructor");
    expect(seriesFill("toString")).toBe("bg-muted-foreground/40");
  });
});
