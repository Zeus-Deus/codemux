import { describe, it, expect } from "vitest";
import { relativeTime } from "./relative-time";

const NOW = new Date("2026-04-29T20:00:00Z");

function ago(seconds: number): Date {
  return new Date(NOW.getTime() - seconds * 1000);
}

describe("relativeTime", () => {
  // ── "just now" bucket ──

  it("returns 'just now' for 0 seconds", () => {
    expect(relativeTime(NOW, NOW)).toBe("just now");
  });

  it("returns 'just now' just under one minute", () => {
    expect(relativeTime(ago(59), NOW)).toBe("just now");
  });

  it("returns 'just now' for clock-skew future dates", () => {
    // Pulled mtime ahead of local clock — render gracefully
    // instead of "-1 minute ago".
    const future = new Date(NOW.getTime() + 30_000);
    expect(relativeTime(future, NOW)).toBe("just now");
  });

  // ── minutes bucket ──

  it("singularises '1 minute ago'", () => {
    expect(relativeTime(ago(60), NOW)).toBe("1 minute ago");
    expect(relativeTime(ago(119), NOW)).toBe("1 minute ago");
  });

  it("pluralises '2 minutes ago'", () => {
    expect(relativeTime(ago(120), NOW)).toBe("2 minutes ago");
    expect(relativeTime(ago(180), NOW)).toBe("3 minutes ago");
  });

  it("upper boundary of minutes bucket is 59 minutes", () => {
    expect(relativeTime(ago(59 * 60), NOW)).toBe("59 minutes ago");
  });

  // ── hours bucket ──

  it("transitions to hours at 60 minutes", () => {
    expect(relativeTime(ago(3600), NOW)).toBe("1 hour ago");
  });

  it("singularises 1 hour, pluralises others", () => {
    expect(relativeTime(ago(3600), NOW)).toBe("1 hour ago");
    expect(relativeTime(ago(2 * 3600), NOW)).toBe("2 hours ago");
    expect(relativeTime(ago(5 * 3600), NOW)).toBe("5 hours ago");
  });

  it("upper boundary of hours bucket is 23 hours", () => {
    expect(relativeTime(ago(23 * 3600), NOW)).toBe("23 hours ago");
  });

  // ── days bucket ──

  it("transitions to days at 24 hours", () => {
    expect(relativeTime(ago(24 * 3600), NOW)).toBe("1 day ago");
  });

  it("singularises 1 day, pluralises others", () => {
    expect(relativeTime(ago(24 * 3600), NOW)).toBe("1 day ago");
    expect(relativeTime(ago(2 * 24 * 3600), NOW)).toBe("2 days ago");
    expect(relativeTime(ago(6 * 24 * 3600), NOW)).toBe("6 days ago");
  });

  // ── absolute fallback ──

  it("falls back to a locale date string for >= 7 days old", () => {
    const oldDate = new Date("2026-04-01T12:00:00Z");
    const result = relativeTime(oldDate, NOW);
    // We can't pin the exact format (locale-dependent) but the
    // result must be the absolute date, not a relative phrase.
    expect(result).not.toContain("ago");
    expect(result).toBe(oldDate.toLocaleDateString());
  });
});
