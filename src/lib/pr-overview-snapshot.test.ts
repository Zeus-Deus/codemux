import { describe, it, expect, beforeEach } from "vitest";

import {
  PR_SNAPSHOT_MAX_ROWS,
  PR_SNAPSHOT_VERSION,
  clearPrOverviewSnapshot,
  prSnapshotKey,
  readPrOverviewSnapshot,
  writePrOverviewSnapshot,
} from "./pr-overview-snapshot";
import type { PrRow } from "./pr-overview";

const ROOT = "/home/dev/projects/codemux";
const OTHER = "/home/dev/projects/site";

function row(over: Partial<PrRow> & { number: number }): PrRow {
  return {
    title: `pull request ${over.number}`,
    author: "juliusm",
    head_branch: `branch/${over.number}`,
    is_draft: false,
    additions: 10,
    deletions: 2,
    review_decision: null,
    checks: "passing",
    review_requested_from: ["mock-dev"],
    updated_at: "2026-08-16T10:00:00Z",
    url: `https://github.com/example/codemux/pull/${over.number}`,
    projectRoot: ROOT,
    repo: "example/codemux",
    providerKind: "github",
    ...over,
  };
}

const viewers = new Map<string, string | null>([[ROOT, "mock-dev"]]);

beforeEach(() => {
  localStorage.clear();
});

describe("prSnapshotKey", () => {
  it("is stable regardless of the order the roots arrive in", () => {
    expect(prSnapshotKey([ROOT, OTHER])).toBe(prSnapshotKey([OTHER, ROOT]));
  });

  it("changes when the set of roots changes", () => {
    // The rule this enforces: close a project and yesterday's rows for
    // it cannot hydrate today's list.
    expect(prSnapshotKey([ROOT])).not.toBe(prSnapshotKey([ROOT, OTHER]));
  });

  it("is namespaced so it can be found and cleared", () => {
    expect(prSnapshotKey([ROOT])).toMatch(/^codemux:pr-overview:v1:/);
  });
});

describe("write → read", () => {
  it("round-trips rows and viewers", () => {
    writePrOverviewSnapshot([ROOT], [row({ number: 1 }), row({ number: 2 })], viewers, 1000);
    const read = readPrOverviewSnapshot([ROOT]);
    expect(read?.savedAt).toBe(1000);
    expect(read?.rows.map((r) => r.number)).toEqual([1, 2]);
    expect(read?.viewerByRoot).toEqual({ [ROOT]: "mock-dev" });
  });

  it("marks everything it reads back as carried", () => {
    writePrOverviewSnapshot([ROOT], [row({ number: 1 })], viewers, 1000);
    expect(readPrOverviewSnapshot([ROOT])?.rows[0].carried).toBe(true);
  });

  it("never stores the carried flag, so an age can't be laundered", () => {
    // A carried row written back would keep claiming the freshness of
    // whatever wrote it; the flag is set on read and stripped on write.
    writePrOverviewSnapshot([ROOT], [row({ number: 1, carried: true })], viewers, 1000);
    const raw = localStorage.getItem(prSnapshotKey([ROOT]))!;
    expect(JSON.parse(raw).rows[0]).not.toHaveProperty("carried");
  });

  it("preserves a null checks value rather than inventing a colour", () => {
    writePrOverviewSnapshot([ROOT], [row({ number: 1, checks: null })], viewers, 1000);
    expect(readPrOverviewSnapshot([ROOT])?.rows[0].checks).toBeNull();
  });

  it("caps at 99 rows", () => {
    const many = Array.from({ length: 150 }, (_, i) => row({ number: i + 1 }));
    writePrOverviewSnapshot([ROOT], many, viewers, 1000);
    const read = readPrOverviewSnapshot([ROOT]);
    expect(read?.rows).toHaveLength(PR_SNAPSHOT_MAX_ROWS);
    expect(read?.rows[PR_SNAPSHOT_MAX_ROWS - 1]?.number).toBe(PR_SNAPSHOT_MAX_ROWS);
  });

  it("does not answer for a different set of roots", () => {
    writePrOverviewSnapshot([ROOT], [row({ number: 1 })], viewers, 1000);
    expect(readPrOverviewSnapshot([ROOT, OTHER])).toBeNull();
  });

  it("returns null when nothing was ever written", () => {
    expect(readPrOverviewSnapshot([ROOT])).toBeNull();
  });
});

describe("validation", () => {
  const put = (value: unknown) =>
    localStorage.setItem(prSnapshotKey([ROOT]), JSON.stringify(value));

  const valid = () => ({
    version: PR_SNAPSHOT_VERSION,
    savedAt: 1000,
    rows: [row({ number: 1 })],
    viewerByRoot: { [ROOT]: "mock-dev" },
  });

  it("rejects an older schema version rather than guessing at it", () => {
    put({ ...valid(), version: PR_SNAPSHOT_VERSION - 1 });
    expect(readPrOverviewSnapshot([ROOT])).toBeNull();
  });

  it("rejects a newer schema version too", () => {
    put({ ...valid(), version: PR_SNAPSHOT_VERSION + 1 });
    expect(readPrOverviewSnapshot([ROOT])).toBeNull();
  });

  it("rejects text that isn't JSON", () => {
    localStorage.setItem(prSnapshotKey([ROOT]), "{not json");
    expect(readPrOverviewSnapshot([ROOT])).toBeNull();
  });

  it("rejects a row missing its identity", () => {
    const bad = valid();
    delete (bad.rows[0] as Partial<PrRow>).projectRoot;
    put(bad);
    expect(readPrOverviewSnapshot([ROOT])).toBeNull();
  });

  it("rejects a row whose field drifted type", () => {
    const bad = valid();
    (bad.rows[0] as unknown as Record<string, unknown>).is_draft = "yes";
    put(bad);
    expect(readPrOverviewSnapshot([ROOT])).toBeNull();
  });

  it("rejects a drifted review_requested_from rather than half-reading it", () => {
    const bad = valid();
    (bad.rows[0] as unknown as Record<string, unknown>).review_requested_from = [1, 2];
    put(bad);
    expect(readPrOverviewSnapshot([ROOT])).toBeNull();
  });

  it("rejects the whole snapshot when one row of many is malformed", () => {
    const bad = valid();
    bad.rows = [row({ number: 1 }), { number: 2 } as PrRow, row({ number: 3 })];
    put(bad);
    expect(readPrOverviewSnapshot([ROOT])).toBeNull();
  });

  it("rejects a bad viewer map", () => {
    const bad = valid();
    (bad as unknown as Record<string, unknown>).viewerByRoot = { [ROOT]: 42 };
    put(bad);
    expect(readPrOverviewSnapshot([ROOT])).toBeNull();
  });

  it("rejects a missing savedAt, which the age label depends on", () => {
    const bad = valid();
    (bad as unknown as Record<string, unknown>).savedAt = null;
    put(bad);
    expect(readPrOverviewSnapshot([ROOT])).toBeNull();
  });

  it("accepts an optional state field", () => {
    put({ ...valid(), rows: [row({ number: 1, state: "OPEN" })] });
    expect(readPrOverviewSnapshot([ROOT])?.rows[0].state).toBe("OPEN");
  });
});

describe("clear", () => {
  it("removes one root set", () => {
    writePrOverviewSnapshot([ROOT], [row({ number: 1 })], viewers, 1000);
    clearPrOverviewSnapshot([ROOT]);
    expect(readPrOverviewSnapshot([ROOT])).toBeNull();
  });

  it("removes every snapshot when asked for none in particular", () => {
    writePrOverviewSnapshot([ROOT], [row({ number: 1 })], viewers, 1000);
    writePrOverviewSnapshot([OTHER], [row({ number: 2, projectRoot: OTHER })], viewers, 1000);
    localStorage.setItem("codemux:unrelated", "keep me");

    clearPrOverviewSnapshot();

    expect(readPrOverviewSnapshot([ROOT])).toBeNull();
    expect(readPrOverviewSnapshot([OTHER])).toBeNull();
    expect(localStorage.getItem("codemux:unrelated")).toBe("keep me");
  });
});
