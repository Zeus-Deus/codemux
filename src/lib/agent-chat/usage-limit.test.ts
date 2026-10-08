import { describe, expect, it } from "vitest";

import type { UsageLimitState } from "./types";
import {
  COARSE_TICK_MS,
  countdownIntervalMs,
  FINE_TICK_MS,
  formatCountdown,
  formatResetTime,
  isAutoResumeText,
  nextUsageLimitBoundary,
  RESUME_GRACE_MS,
  usageLimitPhase,
  usageLimitRecordText,
  usageWindowLabel,
  usageResumeExplanation,
} from "./usage-limit";

const MIN = 60_000;

describe("usageResumeExplanation", () => {
  it("explains an unknown reset in old and new transcripts", () => {
    const expected = "No reset time was reported. The agent won’t continue on its own.";
    expect(usageResumeExplanation(limit(), 0)).toBe(expected);
    expect(usageResumeExplanation(limit({ autoResumeBlockedReason: "unknown_reset" }), 0))
      .toBe(expected);
  });

  it.each([
    ["disabled", "Settings → Agent"],
    ["attempts_exhausted", "Two automatic resumes"],
    ["reset_too_far", "more than 26 hours"],
    ["cancelled", "cancelled"],
    ["dispatch_failed", "couldn’t start"],
    ["storage_failed", "couldn’t be saved"],
    ["reset_passed", "had already passed"],
  ] as const)("explains %s as manual-only without naming a button", (reason, text) => {
    const note = usageResumeExplanation(limit({ autoResumeBlockedReason: reason }), 0);
    expect(note).toContain(text);
    expect(note).toContain("The agent won’t continue on its own.");
    // The same note sits beside Try now before the reset and Resume after.
    expect(note).not.toMatch(/\bResume\b|Try now/);
  });

  it.each(["reset_too_far", "reset_passed"] as const)(
    "keeps the %s note identical on both sides of the reset",
    (reason) => {
      const resetsAtMs = reason === "reset_too_far" ? 27 * 60 * MIN : -MIN;
      const l = limit({ resetsAtMs, autoResumeBlockedReason: reason });
      const before = usageResumeExplanation(l, 0);
      // Hydrated long after the reset, the phase has moved on; the note still holds.
      const after = usageResumeExplanation(l, resetsAtMs + 60 * MIN);
      expect(usageLimitPhase(l, resetsAtMs + 60 * MIN).kind).toBe("reset");
      expect(after).toBe(before);
      // Past tense: never "is … away" or "after it resets" once it has.
      expect(after).toMatch(/\b(was|had already)\b/);
      expect(after).not.toMatch(/\bis more than|after it resets/);
    },
  );

  it("keeps armed countdowns quiet and explains a missed continuation", () => {
    const armed = limit({ autoResumeAtMs: MIN });
    expect(usageResumeExplanation(armed, 0)).toBeNull();
    expect(usageResumeExplanation(armed, MIN + RESUME_GRACE_MS)).toBe(
      "Automatic resume hasn’t started.",
    );
  });
});

function limit(overrides: Partial<UsageLimitState> = {}): UsageLimitState {
  return {
    provider: "claude",
    resetsAtMs: null,
    autoResumeAtMs: null,
    window: null,
    at: 0,
    ...overrides,
  };
}

describe("formatCountdown", () => {
  it("counts seconds inside the last two minutes", () => {
    expect(formatCountdown(0)).toBe("0s");
    expect(formatCountdown(44_200)).toBe("45s");
    expect(formatCountdown(65_000)).toBe("1m 05s");
  });

  it("rounds whole minutes up beyond that", () => {
    expect(formatCountdown(12 * MIN - 1_000)).toBe("12m");
    expect(formatCountdown(72 * MIN)).toBe("1h 12m");
    expect(formatCountdown(120 * MIN)).toBe("2h");
    expect(formatCountdown(51 * 60 * MIN)).toBe("2d 3h");
  });
});

describe("formatResetTime", () => {
  const now = new Date(2026, 8, 18, 12, 0).getTime();

  it("shows only the time for a reset later today", () => {
    expect(formatResetTime(new Date(2026, 8, 18, 23, 40).getTime(), now)).toBe(
      "23:40",
    );
  });

  it("adds the weekday within the coming week", () => {
    const label = formatResetTime(new Date(2026, 8, 20, 9, 5).getTime(), now);
    expect(label).toMatch(/09:05$/);
    expect(label).not.toBe("09:05");
    expect(label).not.toMatch(/20/);
  });

  it("adds the date beyond a week", () => {
    const label = formatResetTime(new Date(2026, 9, 2, 9, 5).getTime(), now);
    expect(label).toMatch(/2, 09:05$/);
  });
});

describe("usage-limit labels", () => {
  it("names the known windows", () => {
    expect(usageWindowLabel("five_hour")).toBe("5-hour limit");
    expect(usageWindowLabel("seven_day")).toBe("weekly limit");
    expect(usageWindowLabel("seven_day_opus")).toBe("weekly Opus limit");
    expect(usageWindowLabel("seven_day_sonnet")).toBe("weekly Sonnet limit");
    expect(usageWindowLabel("overage")).toBe("overage limit");
    expect(usageWindowLabel("mystery")).toBeNull();
    expect(usageWindowLabel(null)).toBeNull();
  });

  it("builds the transcript record from what is known", () => {
    const now = new Date(2026, 8, 18, 12, 0).getTime();
    const reset = new Date(2026, 8, 18, 23, 40).getTime();
    expect(usageLimitRecordText(reset, "five_hour", now)).toBe(
      "Usage limit reached · 5-hour limit · resets 23:40",
    );
    expect(usageLimitRecordText(reset, null, now)).toBe(
      "Usage limit reached · resets 23:40",
    );
    expect(usageLimitRecordText(null, null, now)).toBe("Usage limit reached");
  });

  it("recognises the automatic resume turn by its exact prefix", () => {
    expect(
      isAutoResumeText(
        "[Resumed automatically after a provider usage limit reset. Continue.]",
      ),
    ).toBe(true);
    expect(isAutoResumeText("/goal resume")).toBe(false);
    expect(isAutoResumeText(" [Resumed automatically after a provider usage limit reset")).toBe(false);
  });
});

describe("usageLimitPhase", () => {
  it("armed counts down, then resumes, then falls back to Resume", () => {
    const l = limit({ resetsAtMs: 10 * MIN, autoResumeAtMs: 11 * MIN });
    expect(usageLimitPhase(l, 0)).toEqual({ kind: "armed", at: 11 * MIN });
    expect(usageLimitPhase(l, 11 * MIN)).toEqual({ kind: "resuming" });
    expect(usageLimitPhase(l, 11 * MIN + RESUME_GRACE_MS)).toEqual({ kind: "reset" });
  });

  it("not armed waits on a known reset, else reads reset or reached", () => {
    const l = limit({ resetsAtMs: 10 * MIN });
    expect(usageLimitPhase(l, 0)).toEqual({ kind: "waiting", at: 10 * MIN });
    expect(usageLimitPhase(l, 10 * MIN)).toEqual({ kind: "reset" });
    expect(usageLimitPhase(limit(), 0)).toEqual({ kind: "reached" });
  });

  it("schedules re-renders only at phase and cadence boundaries", () => {
    const armed = limit({ resetsAtMs: 10 * MIN, autoResumeAtMs: 11 * MIN });
    expect(nextUsageLimitBoundary(armed, 0)).toBe(9 * MIN);
    expect(nextUsageLimitBoundary(armed, 9 * MIN)).toBe(11 * MIN);
    expect(nextUsageLimitBoundary(armed, 11 * MIN)).toBe(11 * MIN + RESUME_GRACE_MS);
    expect(nextUsageLimitBoundary(armed, 20 * MIN)).toBeNull();
    expect(nextUsageLimitBoundary(limit(), 0)).toBeNull();
    expect(countdownIntervalMs(10 * MIN, 0)).toBe(COARSE_TICK_MS);
    expect(countdownIntervalMs(10 * MIN, 9 * MIN)).toBe(FINE_TICK_MS);
  });
});
