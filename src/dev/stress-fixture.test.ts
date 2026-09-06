import { describe, it, expect } from "vitest";
import {
  STRESS_PRESETS,
  STRESS_TRANSCRIPT_CACHE_CAPACITY,
  parseStressFixture,
  stressChatTranscript,
} from "./stress-fixture";

describe("parseStressFixture", () => {
  it("returns null for an absent or unrecognised spec, keeping the default seed", () => {
    expect(parseStressFixture(null)).toBeNull();
    expect(parseStressFixture("")).toBeNull();
    expect(parseStressFixture("enormous")).toBeNull();
    expect(parseStressFixture("{not json")).toBeNull();
  });

  it("resolves preset names case-insensitively", () => {
    expect(parseStressFixture("xl")).toEqual(STRESS_PRESETS.xl);
    expect(parseStressFixture(" Large ")).toEqual(STRESS_PRESETS.large);
  });

  it("merges an inline object over the medium preset", () => {
    const fixture = parseStressFixture('{"workspaces":300,"chatEvents":5000}');
    expect(fixture).toEqual({
      workspaces: 300,
      chatEvents: 5_000,
      payloadMb: STRESS_PRESETS.medium.payloadMb,
      deltasPerSec: STRESS_PRESETS.medium.deltasPerSec,
    });
  });

  it("clamps to the ranges the plan calls for", () => {
    const tiny = parseStressFixture('{"workspaces":0,"chatEvents":1}');
    expect(tiny?.workspaces).toBe(1);
    expect(tiny?.chatEvents).toBe(50);

    const huge = parseStressFixture('{"workspaces":9000,"chatEvents":90000}');
    expect(huge?.workspaces).toBe(300);
    expect(huge?.chatEvents).toBe(5_000);

    const nonsense = parseStressFixture('{"workspaces":"many"}');
    expect(nonsense?.workspaces).toBe(1);
  });

  it("provides the audited 270-workspace and 300-workspace release fixtures", () => {
    expect(STRESS_PRESETS.large).toMatchObject({ workspaces: 270, chatEvents: 5_000 });
    expect(STRESS_PRESETS.xl).toMatchObject({ workspaces: 300, chatEvents: 5_000 });
  });
});

describe("stressChatTranscript", () => {
  it("emits exactly the requested number of parseable envelopes", () => {
    const fixture = { workspaces: 1, chatEvents: 200, payloadMb: 0, deltasPerSec: 0 };
    const transcript = stressChatTranscript("thread-stress-count", fixture);
    expect(transcript).toHaveLength(200);
    const kinds = transcript.map((row) => (JSON.parse(row) as { type: string }).type);
    expect(kinds[0]).toBe("user_message");
    expect(new Set(kinds)).toEqual(
      new Set(["user_message", "item_completed", "turn_completed"]),
    );
  });

  it.each(["large", "xl"])("keeps the %s transcript at exactly 5,000 events", (name) => {
    const fixture = STRESS_PRESETS[name];
    const transcript = stressChatTranscript(`thread-stress-${name}-count`, fixture);
    expect(transcript).toHaveLength(5_000);
  });

  it("sizes tool_result payloads to approach the requested megabytes", () => {
    const fixture = { workspaces: 1, chatEvents: 500, payloadMb: 2, deltasPerSec: 0 };
    const transcript = stressChatTranscript("thread-stress-bytes", fixture);
    const bytes = transcript.reduce((sum, row) => sum + row.length, 0);
    const target = 2 * 1024 * 1024;
    expect(bytes).toBeGreaterThan(target * 0.9);
    expect(bytes).toBeLessThan(target * 1.3);
  });

  it("is memoized per thread so a re-hydrate does not rebuild megabytes", () => {
    const fixture = { workspaces: 1, chatEvents: 50, payloadMb: 0, deltasPerSec: 0 };
    const first = stressChatTranscript("thread-stress-memo", fixture);
    expect(stressChatTranscript("thread-stress-memo", fixture)).toBe(first);
  });

  it("bounds the synthetic transcript cache across a workspace sweep", () => {
    const fixture = { workspaces: 1, chatEvents: 50, payloadMb: 0, deltasPerSec: 0 };
    const firstThread = "thread-stress-lru-first";
    const first = stressChatTranscript(firstThread, fixture);
    for (let index = 0; index < STRESS_TRANSCRIPT_CACHE_CAPACITY; index += 1) {
      stressChatTranscript(`thread-stress-lru-${index}`, fixture);
    }
    expect(stressChatTranscript(firstThread, fixture)).not.toBe(first);
  });
});
