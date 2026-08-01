import { describe, expect, it } from "vitest";
import { keepIfUnchanged, sameJson } from "./poll-equality";

describe("sameJson", () => {
  it("treats structurally identical payloads as equal", () => {
    expect(sameJson({ a: 1, b: [2, 3] }, { a: 1, b: [2, 3] })).toBe(true);
    expect(sameJson([], [])).toBe(true);
    expect(sameJson(null, null)).toBe(true);
    expect(sameJson(undefined, undefined)).toBe(true);
  });

  it("detects a moved field", () => {
    expect(sameJson({ a: 1 }, { a: 2 })).toBe(false);
    expect(sameJson([{ path: "a" }], [{ path: "b" }])).toBe(false);
    expect(sameJson(null, undefined)).toBe(false);
  });

  it("is order sensitive, because a reordered list is a re-render anyway", () => {
    expect(sameJson([1, 2], [2, 1])).toBe(false);
  });

  it("reports a cyclic payload as changed rather than throwing", () => {
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    expect(sameJson(cyclic, {})).toBe(false);
  });
});

describe("keepIfUnchanged", () => {
  it("returns the previous reference when the poll returned the same data", () => {
    const prev = [{ path: "src/main.rs", is_staged: true }];
    const next = [{ path: "src/main.rs", is_staged: true }];
    expect(keepIfUnchanged(prev, next)).toBe(prev);
  });

  it("returns the new reference when something moved", () => {
    const prev = [{ path: "src/main.rs", is_staged: true }];
    const next = [{ path: "src/main.rs", is_staged: false }];
    expect(keepIfUnchanged(prev, next)).toBe(next);
  });

  it("handles nullable poll results", () => {
    expect(keepIfUnchanged(null, null)).toBe(null);
    const merge = { is_merging: true };
    expect(keepIfUnchanged(null as typeof merge | null, merge)).toBe(merge);
  });
});
