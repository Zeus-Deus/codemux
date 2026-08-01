import { describe, it, expect } from "vitest";
import {
  STRESS_PRESETS,
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
    const fixture = parseStressFixture('{"workspaces":80,"chatEvents":5000}');
    expect(fixture).toEqual({
      workspaces: 80,
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
    expect(huge?.workspaces).toBe(80);
    expect(huge?.chatEvents).toBe(5_000);

    const nonsense = parseStressFixture('{"workspaces":"many"}');
    expect(nonsense?.workspaces).toBe(1);
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
});
