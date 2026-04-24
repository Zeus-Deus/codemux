import { describe, it, expect } from "vitest";

import type { AgentChatSessionRecord } from "@/tauri/commands";

import {
  groupSessionsByDate,
  parseSqliteTimestamp,
  sessionDisplayTitle,
} from "./session-history";

function makeSession(
  overrides: Partial<AgentChatSessionRecord>,
): AgentChatSessionRecord {
  return {
    thread_id: "thread-1",
    sdk_session_id: null,
    workspace_id: "ws-1",
    cwd: null,
    provider: "claude",
    title: null,
    created_at: "2026-04-24 12:00:00",
    last_active_at: "2026-04-24 12:00:00",
    ...overrides,
  };
}

describe("parseSqliteTimestamp", () => {
  it("parses a SQLite `datetime('now')`-formatted UTC string", () => {
    const ms = parseSqliteTimestamp("2026-04-24 10:30:00");
    expect(new Date(ms).toISOString()).toBe("2026-04-24T10:30:00.000Z");
  });

  it("returns NaN for malformed input", () => {
    expect(Number.isNaN(parseSqliteTimestamp("not-a-date"))).toBe(true);
  });
});

describe("groupSessionsByDate", () => {
  // Pin "now" to 2026-04-24 15:00 UTC so the buckets are deterministic.
  const now = new Date("2026-04-24T15:00:00Z").getTime();

  it("puts today's sessions in the Today bucket", () => {
    const sessions = [
      makeSession({ thread_id: "a", last_active_at: "2026-04-24 10:00:00" }),
      makeSession({ thread_id: "b", last_active_at: "2026-04-24 01:00:00" }),
    ];
    const groups = groupSessionsByDate(sessions, now);
    expect(groups).toHaveLength(1);
    expect(groups[0].key).toBe("today");
    expect(groups[0].label).toBe("Today");
    expect(groups[0].sessions.map((s) => s.thread_id)).toEqual(["a", "b"]);
  });

  it("splits across Today / Yesterday / Last 7 days / Last 30 days / Older", () => {
    const sessions = [
      makeSession({ thread_id: "today", last_active_at: "2026-04-24 10:00:00" }),
      makeSession({ thread_id: "yday", last_active_at: "2026-04-23 23:00:00" }),
      makeSession({ thread_id: "week", last_active_at: "2026-04-20 12:00:00" }),
      makeSession({ thread_id: "month", last_active_at: "2026-04-05 12:00:00" }),
      makeSession({ thread_id: "older", last_active_at: "2025-01-01 12:00:00" }),
    ];
    const groups = groupSessionsByDate(sessions, now);
    expect(groups.map((g) => g.key)).toEqual([
      "today",
      "yesterday",
      "last7",
      "last30",
      "older",
    ]);
    expect(groups.map((g) => g.sessions[0].thread_id)).toEqual([
      "today",
      "yday",
      "week",
      "month",
      "older",
    ]);
  });

  it("omits empty buckets", () => {
    const sessions = [
      makeSession({ thread_id: "today", last_active_at: "2026-04-24 10:00:00" }),
      makeSession({ thread_id: "older", last_active_at: "2025-01-01 12:00:00" }),
    ];
    const groups = groupSessionsByDate(sessions, now);
    expect(groups).toHaveLength(2);
    expect(groups.map((g) => g.key)).toEqual(["today", "older"]);
  });

  it("preserves input order within each bucket", () => {
    // The SQL caller already orders `last_active_at DESC`; the grouper
    // must not reorder.
    const sessions = [
      makeSession({ thread_id: "first", last_active_at: "2026-04-24 14:00:00" }),
      makeSession({ thread_id: "second", last_active_at: "2026-04-24 13:00:00" }),
      makeSession({ thread_id: "third", last_active_at: "2026-04-24 12:00:00" }),
    ];
    const groups = groupSessionsByDate(sessions, now);
    expect(groups[0].sessions.map((s) => s.thread_id)).toEqual([
      "first",
      "second",
      "third",
    ]);
  });

  it("treats malformed timestamps as Older rather than crashing", () => {
    const sessions = [
      makeSession({ thread_id: "broken", last_active_at: "not-a-date" }),
    ];
    const groups = groupSessionsByDate(sessions, now);
    expect(groups).toHaveLength(1);
    expect(groups[0].key).toBe("older");
  });

  it("returns [] for an empty input", () => {
    expect(groupSessionsByDate([], now)).toEqual([]);
  });
});

describe("sessionDisplayTitle", () => {
  it("returns the title verbatim when present", () => {
    const s = makeSession({ title: "Refactor auth" });
    expect(sessionDisplayTitle(s)).toBe("Refactor auth");
  });

  it("falls back to a Chat + thread-id-suffix label when title is null", () => {
    const s = makeSession({ thread_id: "chat-pane-123-1700000000000" });
    expect(sessionDisplayTitle(s)).toBe("Chat 000000");
  });

  it("treats whitespace-only titles as empty and falls back to the id suffix", () => {
    const s = makeSession({
      thread_id: "abc-def-ghij",
      title: "   ",
    });
    // slice(-6) of "abc-def-ghij" = "f-ghij"
    expect(sessionDisplayTitle(s)).toBe("Chat f-ghij");
  });
});
