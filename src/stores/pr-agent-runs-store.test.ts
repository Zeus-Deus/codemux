import { describe, it, expect, beforeEach } from "vitest";
import {
  MAX_RUNS,
  prRefKey,
  selectRunsForPr,
  usePrAgentRunsStore,
  type AgentRunRecord,
} from "./pr-agent-runs-store";

function run(over: Partial<AgentRunRecord> = {}): AgentRunRecord {
  return {
    id: "run-1",
    prRef: "example/codemux#172",
    projectRoot: "/repo",
    prNumber: 172,
    kind: "review-thread",
    summary: "AGENTS.md:12",
    workspaceId: "ws-1",
    workspaceTitle: "drafts",
    threadTabId: "tab-1",
    createdAt: 1_000,
    ...over,
  };
}

beforeEach(() => usePrAgentRunsStore.getState().clear());

describe("the local agent-run history", () => {
  it("finds a run by the host ref both surfaces compute", () => {
    const runs = [run(), run({ id: "other", prRef: "example/other#9", prNumber: 9 })];
    const found = selectRunsForPr(runs, {
      prRef: prRefKey("example/codemux", 172),
      projectRoot: "/elsewhere",
      prNumber: 172,
    });
    expect(found.map((r) => r.id)).toEqual(["run-1"]);
  });

  it("still finds a run whose slug was unknown when it was recorded", () => {
    // The panel may not have resolved a repo slug; the repo path always
    // resolves, so the pair is what keeps the run from being orphaned.
    const runs = [run({ prRef: "#172" })];
    const found = selectRunsForPr(runs, {
      prRef: "example/codemux#172",
      projectRoot: "/repo",
      prNumber: 172,
    });
    expect(found).toHaveLength(1);
  });

  it("does not confuse the same number in a different repository", () => {
    const runs = [run({ prRef: "#172", projectRoot: "/other-repo" })];
    const found = selectRunsForPr(runs, {
      prRef: "example/codemux#172",
      projectRoot: "/repo",
      prNumber: 172,
    });
    expect(found).toEqual([]);
  });

  it("returns runs oldest first, the order the rail reads in", () => {
    const runs = [
      run({ id: "late", createdAt: 3_000 }),
      run({ id: "early", createdAt: 1_000 }),
    ];
    expect(
      selectRunsForPr(runs, { prRef: "example/codemux#172", projectRoot: "/repo", prNumber: 172 })
        .map((r) => r.id),
    ).toEqual(["early", "late"]);
  });

  it("keeps newest-first and never grows past the cap", () => {
    const store = usePrAgentRunsStore.getState();
    for (let i = 0; i < MAX_RUNS + 25; i++) {
      store.record(run({ id: `run-${i}`, createdAt: i }));
    }
    const { runs } = usePrAgentRunsStore.getState();
    expect(runs).toHaveLength(MAX_RUNS);
    // The most recent survived; the oldest were dropped.
    expect(runs[0].id).toBe(`run-${MAX_RUNS + 24}`);
    expect(runs.some((r) => r.id === "run-0")).toBe(false);
  });

  it("re-recording the same id updates rather than duplicates", () => {
    const store = usePrAgentRunsStore.getState();
    store.record(run({ summary: "first" }));
    store.record(run({ summary: "second" }));
    const { runs } = usePrAgentRunsStore.getState();
    expect(runs).toHaveLength(1);
    expect(runs[0].summary).toBe("second");
  });

  it("attaches diff stats only once they are genuinely known", () => {
    const store = usePrAgentRunsStore.getState();
    store.record(run());
    expect(usePrAgentRunsStore.getState().runs[0].files).toBeUndefined();
    store.annotate("run-1", { files: 1, additions: 3, deletions: 0 });
    expect(usePrAgentRunsStore.getState().runs[0]).toMatchObject({
      files: 1,
      additions: 3,
      deletions: 0,
    });
  });
});
