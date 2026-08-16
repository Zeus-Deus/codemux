import { describe, it, expect } from "vitest";

import {
  applyPlan,
  badgeCount,
  badgeKeys,
  badgeLabel,
  freezePlan,
  groupForRow,
  groupRows,
  matchesPrSearch,
  parsePrSearch,
  planOf,
  rowKey,
  type PlanEntry,
  type PrRow,
} from "./pr-overview";

const ROOT = "/home/dev/projects/codemux";
const OTHER_ROOT = "/home/dev/projects/vexis";
const VIEWER = "mock-dev";

function row(over: Partial<PrRow> & { number: number }): PrRow {
  return {
    title: `pr ${over.number}`,
    author: "juliusm",
    head_branch: `branch/${over.number}`,
    is_draft: false,
    additions: 10,
    deletions: 2,
    review_decision: null,
    checks: "passing",
    review_requested_from: [],
    updated_at: new Date(Date.now() - over.number * 60_000).toISOString(),
    url: `https://github.com/example/codemux/pull/${over.number}`,
    projectRoot: ROOT,
    repo: "example/codemux",
    providerKind: "github",
    ...over,
  };
}

const viewers = new Map<string, string | null>([
  [ROOT, VIEWER],
  [OTHER_ROOT, "mock-glab"],
]);

describe("grouping", () => {
  it("files a row by what it wants from you", () => {
    const requested = row({ number: 1, review_requested_from: [VIEWER] });
    const mine = row({ number: 2, author: VIEWER });
    const theirs = row({ number: 3 });

    expect(groupForRow(requested, VIEWER)).toBe("review");
    expect(groupForRow(mine, VIEWER)).toBe("yours");
    expect(groupForRow(theirs, VIEWER)).toBe("watching");
  });

  it("matches the viewer login case-insensitively", () => {
    const mine = row({ number: 4, author: "Mock-Dev" });
    expect(groupForRow(mine, VIEWER)).toBe("yours");
  });

  it("counts a review request on your own PR as needing your review", () => {
    const both = row({ number: 5, author: VIEWER, review_requested_from: [VIEWER] });
    expect(groupForRow(both, VIEWER)).toBe("review");
  });

  it("attributes nothing without a viewer", () => {
    expect(groupForRow(row({ number: 6, author: VIEWER }), null)).toBe("watching");
  });

  it("orders groups review → yours → watching, newest first inside each", () => {
    const rows = [
      row({ number: 30, author: VIEWER }),
      row({ number: 10, review_requested_from: [VIEWER] }),
      row({ number: 20, author: VIEWER }),
      row({ number: 40, projectRoot: OTHER_ROOT, providerKind: "gitlab" }),
    ];
    const groups = groupRows(rows, viewers);
    expect(groups.map((g) => g.id)).toEqual(["review", "yours", "watching"]);
    expect(groups[0].rows.map((r) => r.number)).toEqual([10]);
    // #20 is newer than #30 (the fixture ages by number).
    expect(groups[1].rows.map((r) => r.number)).toEqual([20, 30]);
    expect(groups[2].rows.map((r) => r.number)).toEqual([40]);
  });

  it("resolves the viewer per repository root", () => {
    const rows = [
      // Authored by the GitLab account, in the GitLab root: yours.
      row({ number: 7, projectRoot: OTHER_ROOT, author: "mock-glab" }),
      // Same author, GitHub root, different account: not yours.
      row({ number: 8, author: "mock-glab" }),
    ];
    const groups = groupRows(rows, viewers);
    expect(groups.find((g) => g.id === "yours")?.rows.map((r) => r.number)).toEqual([7]);
    expect(groups.find((g) => g.id === "watching")?.rows.map((r) => r.number)).toEqual([8]);
  });
});

describe("search tokens", () => {
  it("parses is:draft, ci:failing and free text together", () => {
    const search = parsePrSearch("is:draft ci:failing windows shutdown");
    expect(search.isDraft).toBe(true);
    expect(search.ci).toBe("failing");
    expect(search.text).toBe("windows shutdown");
  });

  it("keeps an unrecognised token as text rather than dropping it", () => {
    const search = parsePrSearch("fix: windows");
    expect(search.text).toBe("fix: windows");
    expect(search.ci).toBeNull();
  });

  it("filters by is:draft", () => {
    const draft = row({ number: 1, is_draft: true });
    const ready = row({ number: 2 });
    const search = parsePrSearch("is:draft");
    expect(matchesPrSearch(draft, search)).toBe(true);
    expect(matchesPrSearch(ready, search)).toBe(false);
  });

  it("filters by ci:failing", () => {
    const red = row({ number: 1, checks: "failing" });
    const green = row({ number: 2, checks: "passing" });
    const search = parsePrSearch("ci:failing");
    expect(matchesPrSearch(red, search)).toBe(true);
    expect(matchesPrSearch(green, search)).toBe(false);
  });

  it("combines tokens and free text as an AND", () => {
    const hit = row({ number: 1, is_draft: true, checks: "failing", title: "fix windows shutdown" });
    const wrongText = row({ number: 2, is_draft: true, checks: "failing", title: "docs tidy" });
    const wrongState = row({ number: 3, is_draft: false, checks: "failing", title: "fix windows shutdown" });
    const search = parsePrSearch("is:draft ci:failing windows");
    expect(matchesPrSearch(hit, search)).toBe(true);
    expect(matchesPrSearch(wrongText, search)).toBe(false);
    expect(matchesPrSearch(wrongState, search)).toBe(false);
  });

  it("searches the number, repository and author as well as the title", () => {
    const target = row({ number: 6318, author: "Defmon3", title: "bound child process" });
    expect(matchesPrSearch(target, parsePrSearch("6318"))).toBe(true);
    expect(matchesPrSearch(target, parsePrSearch("defmon3"))).toBe(true);
    expect(matchesPrSearch(target, parsePrSearch("codemux"))).toBe(true);
  });
});

describe("badge", () => {
  const rows = [
    row({ number: 1, review_requested_from: [VIEWER] }),
    row({ number: 2, review_requested_from: [VIEWER] }),
    row({ number: 3, author: VIEWER, checks: "failing" }),
    // Yours and green: not a badge.
    row({ number: 4, author: VIEWER, checks: "passing" }),
    // Someone else's red PR: not yours to fix.
    row({ number: 5, checks: "failing" }),
  ];

  it("counts review requests plus your own failing checks", () => {
    expect(badgeCount(badgeKeys(rows, viewers), new Set())).toBe(3);
  });

  it("counts a PR once even when it is both", () => {
    const both = [
      ...rows,
      row({ number: 6, author: VIEWER, checks: "failing", review_requested_from: [VIEWER] }),
    ];
    const keys = badgeKeys(both, viewers);
    expect(new Set(keys).size).toBe(keys.length);
    expect(badgeCount(keys, new Set())).toBe(4);
  });

  it("counts nothing without a viewer for that root", () => {
    const unknown = [row({ number: 9, projectRoot: "/nowhere", review_requested_from: [VIEWER] })];
    expect(badgeKeys(unknown, viewers)).toEqual([]);
  });

  it("clears once the page has shown them, and re-raises for new ones", () => {
    const keys = badgeKeys(rows, viewers);
    const seen = new Set(keys);
    expect(badgeCount(keys, seen)).toBe(0);

    const later = [...rows, row({ number: 7, review_requested_from: [VIEWER] })];
    expect(badgeCount(badgeKeys(later, viewers), seen)).toBe(1);
  });

  it("caps the label at 9+", () => {
    expect(badgeLabel(3)).toBe("3");
    expect(badgeLabel(9)).toBe("9");
    expect(badgeLabel(10)).toBe("9+");
    expect(badgeLabel(58)).toBe("9+");
  });
});

describe("rule 03 — the ordering freeze", () => {
  const plan = (...keys: string[]): PlanEntry[] =>
    keys.map((key) => ({ key, groupId: "yours" as const }));

  it("holds the old order and marks what moved", () => {
    const before = plan("a", "b", "c");
    const after = plan("c", "a", "b");
    const frozen = freezePlan(after, before);
    expect(frozen.entries.map((e) => e.key)).toEqual(["a", "b", "c"]);
    expect(frozen.moved.size).toBeGreaterThan(0);
  });

  it("marks a row that changed group without moving it", () => {
    const before: PlanEntry[] = [{ key: "a", groupId: "watching" }];
    const after: PlanEntry[] = [{ key: "a", groupId: "review" }];
    const frozen = freezePlan(after, before);
    expect(frozen.entries).toEqual([{ key: "a", groupId: "watching" }]);
    expect(frozen.moved.has("a")).toBe(true);
  });

  it("appends arrivals rather than inserting them mid-list", () => {
    const before = plan("a", "b");
    const after = plan("new", "a", "b");
    const frozen = freezePlan(after, before);
    expect(frozen.entries.map((e) => e.key)).toEqual(["a", "b", "new"]);
    expect(frozen.moved.has("new")).toBe(true);
  });

  it("drops rows the host no longer has", () => {
    const frozen = freezePlan(plan("a"), plan("a", "gone"));
    expect(frozen.entries.map((e) => e.key)).toEqual(["a"]);
  });

  it("applies the new order with no snapshot to hold", () => {
    const after = plan("c", "a");
    expect(freezePlan(after, null)).toEqual({ entries: after, moved: new Set() });
  });

  it("round-trips through planOf/applyPlan", () => {
    const rows = [
      row({ number: 1, review_requested_from: [VIEWER] }),
      row({ number: 2, author: VIEWER }),
      row({ number: 3 }),
    ];
    const groups = groupRows(rows, viewers);
    const rebuilt = applyPlan(rows, planOf(groups));
    expect(rebuilt.map((g) => g.rows.map(rowKey))).toEqual(
      groups.map((g) => g.rows.map(rowKey)),
    );
  });
});
