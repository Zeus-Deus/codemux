import { afterAll, beforeEach, describe, expect, it } from "vitest";
import {
  computeSnoozePresets,
  formatTimeUntil,
  formatWakeLabel,
} from "./sidebar-snooze";

const HOUR = 3_600_000;
const NAIVE_DAY = 86_400_000;

// The repo's vitest config does not pin a timezone, so every date-sensitive
// assertion below would otherwise depend on whichever zone the CI machine
// happens to run in. Each block pins its own zone; Node re-reads `process.env.TZ`
// on the next Date operation, so this takes effect per test rather than only at
// process start.
const ORIGINAL_TZ = process.env.TZ;

function pinTimeZone(tz: string): void {
  beforeEach(() => {
    process.env.TZ = tz;
  });
}

afterAll(() => {
  // Leaking a TZ override would silently retune every other test file that
  // shares this worker process.
  if (ORIGINAL_TZ === undefined) delete process.env.TZ;
  else process.env.TZ = ORIGINAL_TZ;
});

/** Local wall-clock instant, with a human month (1-12). Must be called inside a
 *  test so the pinned zone is already in effect. */
function local(
  year: number,
  month: number,
  day: number,
  hour: number,
  minute = 0,
): number {
  return new Date(year, month - 1, day, hour, minute, 0, 0).getTime();
}

function byId(now: number) {
  return new Map(computeSnoozePresets(now).map((p) => [p.id, p]));
}

describe("computeSnoozePresets", () => {
  pinTimeZone("America/New_York");

  it("offers all four presets at a normal midday moment", () => {
    // Wednesday 2026-06-10, 12:00 local.
    const now = local(2026, 6, 10, 12);
    const presets = computeSnoozePresets(now);

    expect(presets.map((p) => p.id)).toEqual([
      "one-hour",
      "this-evening",
      "tomorrow",
      "next-week",
    ]);
    expect(presets.map((p) => p.label)).toEqual([
      "In 1 hour",
      "This evening",
      "Tomorrow",
      "Next week",
    ]);

    const at = byId(now);
    expect(at.get("one-hour")?.at).toBe(now + HOUR);
    expect(at.get("this-evening")?.at).toBe(local(2026, 6, 10, 18));
    expect(at.get("tomorrow")?.at).toBe(local(2026, 6, 11, 9));
    // Wednesday → the coming Monday.
    expect(at.get("next-week")?.at).toBe(local(2026, 6, 15, 9));
  });

  it("keeps 'in 1 hour' available at every hour of the day", () => {
    for (const hour of [0, 6, 12, 17, 23]) {
      const now = local(2026, 6, 10, hour);
      expect(byId(now).has("one-hour")).toBe(true);
    }
  });

  it("suppresses 'this evening' at 17:30 (would fire almost immediately)", () => {
    const now = local(2026, 6, 10, 17, 30);
    const presets = computeSnoozePresets(now);
    expect(presets.map((p) => p.id)).toEqual([
      "one-hour",
      "tomorrow",
      "next-week",
    ]);
  });

  it("suppresses 'this evening' at 23:00 (already passed)", () => {
    const now = local(2026, 6, 10, 23);
    const presets = computeSnoozePresets(now);
    expect(presets.map((p) => p.id)).toEqual([
      "one-hour",
      "tomorrow",
      "next-week",
    ]);
    // Crucially it is not offered with a wake time in the past, which would
    // make a snooze that never comes back.
    expect(presets.every((p) => p.at > now)).toBe(true);
  });

  it("still offers 'this evening' at exactly one hour out", () => {
    const now = local(2026, 6, 10, 17);
    expect(byId(now).get("this-evening")?.at).toBe(local(2026, 6, 10, 18));
  });

  it("puts 'next week' a full 7 days out when today is Monday", () => {
    // Monday 2026-06-08 — the next Monday is the 15th, not today.
    const now = local(2026, 6, 8, 10);
    const nextWeek = byId(now).get("next-week");
    expect(nextWeek?.at).toBe(local(2026, 6, 15, 9));
    expect(new Date(nextWeek!.at).getDay()).toBe(1);
  });

  it("puts 'next week' on the coming Monday when today is Saturday", () => {
    // Saturday 2026-06-13 → Monday the 15th.
    const now = local(2026, 6, 13, 10);
    const nextWeek = byId(now).get("next-week");
    expect(nextWeek?.at).toBe(local(2026, 6, 15, 9));
    expect(new Date(nextWeek!.at).getDay()).toBe(1);
  });

  it("drops 'next week' on a Sunday, when the coming Monday IS tomorrow", () => {
    // Sunday 2026-06-14: next Monday is the 15th — the same wake instant the
    // "Tomorrow" preset already offers. Two entries resolving to one moment
    // would read as a menu bug ("Next week · Tomorrow 09:00"), so the
    // redundant one is withheld.
    const now = local(2026, 6, 14, 10);
    const presets = computeSnoozePresets(now);
    expect(presets.map((p) => p.id)).toEqual([
      "one-hour",
      "this-evening",
      "tomorrow",
    ]);
    expect(byId(now).get("tomorrow")?.at).toBe(local(2026, 6, 15, 9));
  });

  it("keeps 'next week' distinct from 'tomorrow' on every other weekday", () => {
    // 2026-06-08 is a Monday; each subsequent day through Saturday must keep
    // both presets, at least a calendar day apart.
    for (let day = 8; day <= 13; day++) {
      const now = local(2026, 6, day, 10);
      const at = byId(now);
      const tomorrow = at.get("tomorrow");
      const nextWeek = at.get("next-week");
      expect(tomorrow).toBeDefined();
      expect(nextWeek).toBeDefined();
      expect(nextWeek!.at - tomorrow!.at).toBeGreaterThanOrEqual(NAIVE_DAY - HOUR);
    }
  });
});

// The suite does not pin a locale (the helper deliberately follows the user's),
// so assertions match the hour digits either a 12- or a 24-hour locale renders
// — "9:00 AM" and "09:00" alike — rather than one machine's clock convention.
const NINE_AM = /\b0?9[:.]00/;
const SIX_PM = /\b(0?6|18)[:.]00/;

describe("formatWakeLabel", () => {
  pinTimeZone("America/New_York");

  it("gives a wake later today the bare time — the date would be noise", () => {
    const now = local(2026, 6, 10, 12);
    const label = formatWakeLabel(now, local(2026, 6, 10, 18));
    expect(label).toMatch(SIX_PM);
    expect(label).not.toMatch(/Tomorrow|Wed|Thu/);
  });

  it("names tomorrow as tomorrow", () => {
    const now = local(2026, 6, 10, 12);
    const label = formatWakeLabel(now, local(2026, 6, 11, 9));
    expect(label).toMatch(/^Tomorrow /);
    expect(label).toMatch(NINE_AM);
  });

  it("names anything further out by weekday, which is what people plan on", () => {
    const now = local(2026, 6, 10, 12); // Wednesday
    const label = formatWakeLabel(now, local(2026, 6, 15, 9)); // Monday
    expect(label).toMatch(/^Mon /);
    expect(label).toMatch(NINE_AM);
  });

  it("reads 'Tomorrow' for an hour-from-now that crosses midnight", () => {
    // The case the absolute label exists for: "In 1 hour" at 23:30 is not a
    // wake today, and only the concrete time says so.
    const now = local(2026, 6, 10, 23, 30);
    expect(formatWakeLabel(now, now + HOUR)).toMatch(/^Tomorrow /);
  });

  it("still calls a 23-hour day tomorrow (spring forward)", () => {
    // 2026-03-08 is 23 hours long in New York, so the raw gap from noon
    // Saturday to 09:00 Sunday is under a day — day *starts* are what decide.
    const now = local(2026, 3, 7, 12);
    expect(formatWakeLabel(now, local(2026, 3, 8, 9))).toMatch(/^Tomorrow /);
  });

  it("still calls a 25-hour day tomorrow (fall back)", () => {
    const now = local(2026, 10, 31, 12);
    expect(formatWakeLabel(now, local(2026, 11, 1, 9))).toMatch(/^Tomorrow /);
  });
});

describe("computeSnoozePresets — whenLabel", () => {
  pinTimeZone("America/New_York");

  it("labels every preset with the instant it actually fires", () => {
    const now = local(2026, 6, 10, 12); // Wednesday
    const at = byId(now);
    expect(at.get("one-hour")?.whenLabel).toMatch(/\b0?1[:.]00/);
    expect(at.get("this-evening")?.whenLabel).toMatch(SIX_PM);
    // The "Tomorrow" row drops the day prefix its own label already carries —
    // "Tomorrow · Tomorrow 09:00" reads as a bug.
    expect(at.get("tomorrow")?.whenLabel).not.toMatch(/Tomorrow/);
    expect(at.get("tomorrow")?.whenLabel).toMatch(NINE_AM);
    expect(at.get("next-week")?.whenLabel).toMatch(/^Mon /);
  });

  it("keeps the label and the wake time the same instant", () => {
    // One source of truth: a label that could drift from `at` would promise a
    // return the sweep has no intention of honouring. Only the "Tomorrow" row
    // is allowed to differ, and only by dropping a redundant day prefix.
    const now = local(2026, 6, 10, 12);
    for (const preset of computeSnoozePresets(now)) {
      const full = formatWakeLabel(now, preset.at);
      const expected =
        preset.id === "tomorrow" ? full.replace(/^Tomorrow\s+/, "") : full;
      expect(preset.whenLabel).toBe(expected);
      // Whatever the prefix, the clock time itself must survive intact.
      expect(full).toContain(preset.whenLabel);
    }
  });

  it("still names the day when the relative label does not", () => {
    // "In 1 hour" taken late at night crosses midnight — stripping its prefix
    // the way the Tomorrow row does would hide a whole day.
    const at = byId(local(2026, 6, 10, 23, 30));
    expect(at.get("one-hour")?.whenLabel).toMatch(/^Tomorrow /);
  });

  it("labels 09:00 across a spring-forward rather than 08:00", () => {
    const now = local(2026, 3, 7, 12);
    const at = byId(now);
    expect(at.get("tomorrow")?.whenLabel).toMatch(NINE_AM);
    expect(at.get("next-week")?.whenLabel).toMatch(NINE_AM);
  });

  it("labels 09:00 across a fall-back rather than 10:00", () => {
    const now = local(2026, 10, 31, 12);
    const at = byId(now);
    expect(at.get("tomorrow")?.whenLabel).toMatch(NINE_AM);
    expect(at.get("next-week")?.whenLabel).toMatch(NINE_AM);
  });
});

describe("computeSnoozePresets across DST boundaries", () => {
  pinTimeZone("America/New_York");

  it("lands at 09:00 local across a spring-forward (23-hour day)", () => {
    // Saturday 2026-03-07 12:00 EST; the clocks jump forward on the 8th, so
    // that day is only 23 hours long.
    const now = local(2026, 3, 7, 12);

    const at = byId(now);
    const tomorrow = new Date(at.get("tomorrow")!.at);
    expect(tomorrow.getDate()).toBe(8);
    expect(tomorrow.getHours()).toBe(9);
    expect(tomorrow.getMinutes()).toBe(0);
    // 21 wall-clock hours, but only 20 real hours: proof the wake time was
    // built from calendar components, not ms arithmetic.
    expect(at.get("tomorrow")!.at - now).toBe(20 * HOUR);

    const nextWeek = new Date(at.get("next-week")!.at);
    expect(nextWeek.getDate()).toBe(9);
    expect(nextWeek.getHours()).toBe(9);
    expect(at.get("next-week")!.at - now).toBe(44 * HOUR);

    // What a naive `now + 86_400_000` would have produced instead.
    expect(new Date(now + NAIVE_DAY).getHours()).toBe(13);
  });

  it("lands at 09:00 local across a fall-back (25-hour day)", () => {
    // Saturday 2026-10-31 12:00 EDT; the clocks fall back on Nov 1, making
    // that day 25 hours long.
    const now = local(2026, 10, 31, 12);

    const at = byId(now);
    const tomorrow = new Date(at.get("tomorrow")!.at);
    expect(tomorrow.getMonth()).toBe(10); // November
    expect(tomorrow.getDate()).toBe(1);
    expect(tomorrow.getHours()).toBe(9);
    expect(at.get("tomorrow")!.at - now).toBe(22 * HOUR);

    const nextWeek = new Date(at.get("next-week")!.at);
    expect(nextWeek.getDate()).toBe(2);
    expect(nextWeek.getHours()).toBe(9);
    expect(at.get("next-week")!.at - now).toBe(46 * HOUR);

    expect(new Date(now + NAIVE_DAY).getHours()).toBe(11);
  });
});

describe("computeSnoozePresets in a non-US timezone", () => {
  // Europe/Berlin changes on a different weekend than the US, so this also
  // guards against a fix that only happened to work for one zone's rules.
  pinTimeZone("Europe/Berlin");

  it("lands at 09:00 local across Berlin's spring-forward", () => {
    // Saturday 2026-03-28 12:00 CET; the clocks jump forward on the 29th.
    const now = local(2026, 3, 28, 12);
    const tomorrow = byId(now).get("tomorrow")!;
    expect(new Date(tomorrow.at).getDate()).toBe(29);
    expect(new Date(tomorrow.at).getHours()).toBe(9);
    expect(tomorrow.at - now).toBe(20 * HOUR);
  });
});

describe("formatTimeUntil", () => {
  it("reads 'now' at zero and for an overdue wake", () => {
    expect(formatTimeUntil(0)).toBe("now");
    expect(formatTimeUntil(-1)).toBe("now");
    expect(formatTimeUntil(-3 * HOUR)).toBe("now");
  });

  it("counts seconds under a minute rather than rounding to '0m'", () => {
    expect(formatTimeUntil(1_000)).toBe("1s");
    expect(formatTimeUntil(45_000)).toBe("45s");
    expect(formatTimeUntil(59_999)).toBe("59s");
  });

  it("counts minutes under an hour", () => {
    expect(formatTimeUntil(60_000)).toBe("1m");
    expect(formatTimeUntil(90_000)).toBe("1m");
    expect(formatTimeUntil(30 * 60_000)).toBe("30m");
    expect(formatTimeUntil(59 * 60_000)).toBe("59m");
  });

  it("counts hours (plus minutes) under a day", () => {
    expect(formatTimeUntil(HOUR)).toBe("1h");
    expect(formatTimeUntil(2 * HOUR)).toBe("2h");
    expect(formatTimeUntil(HOUR + 12 * 60_000)).toBe("1h12m");
    expect(formatTimeUntil(18 * HOUR)).toBe("18h");
    expect(formatTimeUntil(23 * HOUR + 59 * 60_000)).toBe("23h59m");
  });

  it("counts days (plus hours) past a day", () => {
    expect(formatTimeUntil(24 * HOUR)).toBe("1d");
    expect(formatTimeUntil(27 * HOUR)).toBe("1d3h");
    expect(formatTimeUntil(3 * NAIVE_DAY)).toBe("3d");
    expect(formatTimeUntil(7 * NAIVE_DAY)).toBe("7d");
  });

  it("never renders a preset's countdown as a negative duration", () => {
    const now = Date.parse("2026-06-10T12:00:00Z");
    for (const preset of computeSnoozePresets(now)) {
      expect(formatTimeUntil(preset.at - now)).not.toMatch(/^-/);
    }
  });
});
