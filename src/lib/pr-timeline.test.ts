import { describe, it, expect } from "vitest";
import { buildTimeline, summarizeChecks, type TimelineEntry } from "./pr-timeline";
import type { AgentRunRecord } from "@/stores/pr-agent-runs-store";
import type { CheckInfo, PrTimelineEvent, PullRequestInfo } from "@/tauri/types";

const T0 = Date.parse("2026-08-16T09:00:00Z");
const at = (minutes: number) => new Date(T0 + minutes * 60_000).toISOString();

function pr(over: Partial<PullRequestInfo> = {}): PullRequestInfo {
  return {
    number: 172,
    url: "https://github.com/example/codemux/pull/172",
    state: "OPEN",
    title: "feat: drafts",
    head_branch: "agent/drafts",
    base_branch: "main",
    is_draft: false,
    mergeable: "MERGEABLE",
    additions: 10,
    deletions: 2,
    review_decision: null,
    checks_passing: null,
    updated_at: at(60),
    created_at: at(0),
    body: null,
    comments: [],
    totalComments: 0,
    author: "Zeus-Deus",
    head_ref_oid: "abc",
    head_repository_owner: "example",
    merge_state_status: "CLEAN",
    changed_files: 3,
    merged_by: null,
    merged_at: null,
    review_requests: [],
    latest_reviews: [],
    ...over,
  };
}

const HOST_EVENTS: PrTimelineEvent[] = [
  {
    id: "e1",
    actor: "juliusm",
    created_at: at(30),
    kind: "reviewed",
    verdict: "CHANGES_REQUESTED",
    body: "Worth a line here.",
    anchor: "AGENTS.md:12",
  },
  {
    id: "e2",
    actor: "Zeus-Deus",
    created_at: at(90),
    kind: "committed",
    sha: "a1f9c2e",
    message: "docs: add the note",
  },
];

function run(over: Partial<AgentRunRecord> = {}): AgentRunRecord {
  return {
    id: "run-1",
    prRef: "example/codemux#172",
    projectRoot: "/repo",
    prNumber: 172,
    kind: "review-thread",
    summary: "AGENTS.md:12",
    workspaceId: "ws-1",
    workspaceTitle: "this workspace",
    threadTabId: "tab-9",
    // Deliberately between the two host events.
    createdAt: T0 + 60 * 60_000,
    ...over,
  };
}

function check(name: string, conclusion: string): CheckInfo {
  return {
    name,
    status: conclusion === "pending" ? "IN_PROGRESS" : "COMPLETED",
    conclusion,
    elapsed_time: "1m",
    detail_url: null,
    started_at: null,
    completed_at: null,
  };
}

const ids = (entries: TimelineEntry[]) => entries.map((e) => e.id);

describe("the timeline model", () => {
  it("puts a local agent run between the host events it sits between", () => {
    const entries = buildTimeline({
      pr: pr(),
      events: HOST_EVENTS,
      runs: [run()],
      checks: [],
      filter: "everything",
    });

    // Opened is synthesized and always leads; the run interleaves by its
    // own timestamp rather than being appended to either group.
    expect(ids(entries)).toEqual(["opened", "e1", "run-1", "e2"]);
    expect(entries[2].type).toBe("agent");
  });

  it("removes agent runs under Host only, and moves nothing else", () => {
    const args = {
      pr: pr(),
      events: HOST_EVENTS,
      runs: [run()],
      checks: [] as CheckInfo[],
    };

    const everything = buildTimeline({ ...args, filter: "everything" });
    const hostOnly = buildTimeline({ ...args, filter: "host" });

    expect(ids(hostOnly)).toEqual(["opened", "e1", "e2"]);
    // The point of the filter: what a teammate would see, with the host
    // entries in exactly the order they were already in.
    expect(hostOnly.every((e) => e.type !== "agent")).toBe(true);
    expect(ids(everything).filter((id) => id !== "run-1")).toEqual(ids(hostOnly));
  });

  it("synthesizes the opened row the host never sends, counting commits from the history", () => {
    const entries = buildTimeline({
      pr: pr(),
      events: HOST_EVENTS,
      runs: [],
      checks: [],
      filter: "everything",
    });
    const opened = entries[0];
    expect(opened.type).toBe("host");
    if (opened.type !== "host") throw new Error("unreachable");
    expect(opened.event.kind).toBe("opened");
    if (opened.event.kind !== "opened") throw new Error("unreachable");
    expect(opened.event.commits).toBe(1);
    expect(opened.event.actor).toBe("Zeus-Deus");
  });

  it("keeps an undated host event in place instead of hoisting or dropping it", () => {
    const entries = buildTimeline({
      pr: pr(),
      events: [
        HOST_EVENTS[0],
        { id: "undated", actor: null, created_at: null, kind: "closed" },
        HOST_EVENTS[1],
      ],
      runs: [],
      checks: [],
      filter: "everything",
    });
    expect(ids(entries)).toEqual(["opened", "e1", "undated", "e2"]);
  });
});

describe("the synthesized checks row", () => {
  it("reads the live checks query rather than the timeline payload", () => {
    const entries = buildTimeline({
      pr: pr(),
      // The host history says nothing at all about checks…
      events: HOST_EVENTS,
      runs: [],
      // …and the row still reflects what the checks query returned.
      checks: [
        check("build", "pass"),
        check("lint", "pass"),
        check("test", "pass"),
        check("clippy", "pass"),
        check("e2e", "pending"),
      ],
      filter: "everything",
    });

    const last = entries[entries.length - 1];
    expect(last.type).toBe("checks");
    if (last.type !== "checks") throw new Error("unreachable");
    expect(last.checks.sentence).toBe("4 checks passed, 1 running");
    expect(last.checks.spinning).toBe(true);
  });

  it("is always last, even though its timestamp is now", () => {
    const entries = buildTimeline({
      pr: pr(),
      events: HOST_EVENTS,
      runs: [run()],
      checks: [check("build", "pass")],
      filter: "everything",
    });
    expect(entries[entries.length - 1].type).toBe("checks");
  });

  it("is absent entirely when there are no checks", () => {
    const entries = buildTimeline({
      pr: pr(),
      events: HOST_EVENTS,
      runs: [],
      checks: [],
      filter: "everything",
    });
    expect(entries.some((e) => e.type === "checks")).toBe(false);
  });

  it("names only the buckets that have something in them", () => {
    expect(summarizeChecks([check("a", "pass"), check("b", "pass")])?.sentence).toBe(
      "2 checks passed",
    );
    expect(
      summarizeChecks([check("a", "pass"), check("b", "fail")])?.sentence,
    ).toBe("1 check passed, 1 failed");
    expect(summarizeChecks([])).toBeNull();
  });
});
