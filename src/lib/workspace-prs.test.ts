import { describe, expect, it } from "vitest";
import {
  prSetLabel,
  prSetSummary,
  prsDescribeThisCheckout,
  stackOrder,
  stackedOn,
  workspacePrs,
  type WorkspacePrRef,
} from "./workspace-prs";
import { providerForWorkspace } from "./source-control";
import type { WorkspaceSnapshot } from "@/tauri/types";

const github = providerForWorkspace({ provider_kind: "github" });

function pr(
  number: number,
  state: string,
  head?: string,
  base?: string,
): WorkspacePrRef {
  return {
    number,
    state,
    url: `https://github.com/u/r/pull/${number}`,
    head_branch: head ?? `branch-${number}`,
    base_branch: base ?? "main",
  };
}

/** The real shape from the nine-PR workspace that motivated this: one PR on
 *  `main` and eight chained on their predecessors. */
function stackOfNine(states: string[]): WorkspacePrRef[] {
  return states.map((state, i) =>
    pr(
      372 + i,
      state,
      `ui-pass/0${i + 1}`,
      i === 0 ? "main" : `ui-pass/0${i}`,
    ),
  );
}

type PrFields = Pick<
  WorkspaceSnapshot,
  "prs" | "pr_number" | "pr_state" | "pr_url" | "pr_head_branch"
>;

const scalarsOnly: PrFields = {
  prs: undefined,
  pr_number: 371,
  pr_state: "OPEN",
  pr_url: "https://github.com/u/r/pull/371",
  pr_head_branch: "fix/hosted-signin-context",
};

describe("workspacePrs", () => {
  it("reads the set when the backend supplies one", () => {
    const prs = stackOfNine(Array(9).fill("OPEN"));
    expect(
      workspacePrs({ ...scalarsOnly, prs }).map((p) => p.number),
    ).toEqual([372, 373, 374, 375, 376, 377, 378, 379, 380]);
  });

  it("falls back to the scalars for a snapshot persisted before the field", () => {
    // Not dead code: this is also every provider whose set discovery is the
    // default single-PR delegation.
    expect(workspacePrs(scalarsOnly)).toEqual([
      {
        number: 371,
        state: "OPEN",
        url: "https://github.com/u/r/pull/371",
        head_branch: "fix/hosted-signin-context",
      },
    ]);
  });

  it("reports no PRs when the scalars are empty", () => {
    expect(
      workspacePrs({
        prs: [],
        pr_number: null,
        pr_state: null,
        pr_url: null,
        pr_head_branch: null,
      }),
    ).toEqual([]);
  });
});

describe("prSetSummary", () => {
  it("counts each state across the set", () => {
    const summary = prSetSummary(
      stackOfNine([
        "MERGED",
        "MERGED",
        "MERGED",
        "MERGED",
        "OPEN",
        "OPEN",
        "OPEN",
        "OPEN",
        "OPEN",
      ]),
    );
    expect(summary.total).toBe(9);
    expect(summary.merged).toBe(4);
    expect(summary.open).toBe(5);
  });

  it("reads a partly merged stack as open, not merged", () => {
    // The bug this exists to prevent: reporting the set by its earliest PR
    // settles the card out of the sidebar with five reviews still to come.
    const summary = prSetSummary(
      stackOfNine([
        "MERGED",
        "MERGED",
        "MERGED",
        "MERGED",
        "OPEN",
        "OPEN",
        "OPEN",
        "OPEN",
        "OPEN",
      ]),
    );
    expect(summary.state).toBe("open");
    expect(summary.allSettled).toBe(false);
  });

  it("settles only once every PR is terminal", () => {
    const summary = prSetSummary(stackOfNine(Array(9).fill("MERGED")));
    expect(summary.state).toBe("merged");
    expect(summary.allSettled).toBe(true);
  });

  it("ranks a draft above the terminal states but below open", () => {
    expect(prSetSummary([pr(1, "DRAFT"), pr(2, "MERGED")]).state).toBe("draft");
    expect(prSetSummary([pr(1, "DRAFT"), pr(2, "OPEN")]).state).toBe("open");
  });

  it("reads a partly landed, partly abandoned stack as merged", () => {
    expect(prSetSummary([pr(1, "MERGED"), pr(2, "CLOSED")]).state).toBe(
      "merged",
    );
    expect(prSetSummary([pr(1, "CLOSED"), pr(2, "CLOSED")]).state).toBe(
      "closed",
    );
  });

  it("has no state for an empty set", () => {
    const summary = prSetSummary([]);
    expect(summary.state).toBeNull();
    expect(summary.allSettled).toBe(false);
  });
});

describe("prsDescribeThisCheckout", () => {
  it("trusts a set of more than one even when HEAD is on none of them", () => {
    // Exactly the stack case: the agent cut nine branches with `git branch`
    // and left the worktree on its original branch. The side-branch fallback
    // can only ever contribute one PR, so anything larger came from
    // worktree-owned discovery and is this workspace's own work.
    const prs = stackOfNine(Array(9).fill("MERGED"));
    expect(prsDescribeThisCheckout(prs, "goal-passpage-space-task")).toBe(true);
  });

  it("still guards a lone side-branch association", () => {
    expect(
      prsDescribeThisCheckout([pr(1, "MERGED", "side-branch")], "my-branch"),
    ).toBe(false);
    expect(
      prsDescribeThisCheckout([pr(1, "MERGED", "my-branch")], "my-branch"),
    ).toBe(true);
  });

  it("treats an unknown branch on either side as a match", () => {
    // A detached HEAD during a rebase must not un-associate a workspace from
    // its own PR for the length of the rebase.
    expect(prsDescribeThisCheckout([pr(1, "OPEN", "b")], null)).toBe(true);
    expect(prsDescribeThisCheckout([], "my-branch")).toBe(true);
  });
});

describe("stackedOn", () => {
  it("links a PR to the one it is based on", () => {
    const prs = stackOfNine(Array(9).fill("OPEN"));
    expect(stackedOn(prs[0], prs)).toBeNull();
    expect(stackedOn(prs[1], prs)?.number).toBe(372);
    expect(stackedOn(prs[8], prs)?.number).toBe(379);
  });

  it("does not invent a link for a PR based on a branch outside the set", () => {
    const prs = [pr(1, "OPEN", "feature", "main")];
    expect(stackedOn(prs[0], prs)).toBeNull();
  });
});

describe("prSetLabel", () => {
  it("names the single PR it opens", () => {
    const prs = [pr(371, "OPEN")];
    expect(prSetLabel(github, prs, prSetSummary(prs))).toBe(
      "Pull request #371 — open",
    );
  });

  it("reads out the composition of a set", () => {
    const prs = stackOfNine([
      "MERGED",
      "MERGED",
      "MERGED",
      "MERGED",
      "OPEN",
      "OPEN",
      "OPEN",
      "OPEN",
      "OPEN",
    ]);
    expect(prSetLabel(github, prs, prSetSummary(prs))).toBe(
      "9 pull requests — 5 open, 4 merged. Opens #372",
    );
  });

  it("uses the caller's state for an association with no number", () => {
    expect(prSetLabel(github, [], prSetSummary([]), "open")).toBe(
      "Pull request — open",
    );
  });
});

describe("stackOrder", () => {
  it("rebuilds bottom-up order from base/head links, not incoming order", () => {
    const stack = stackOfNine(Array(9).fill("OPEN"));
    const shuffled = [stack[4], stack[8], stack[0], stack[2], stack[1], stack[7], stack[3], stack[6], stack[5]];
    expect(stackOrder(shuffled).map((p) => p.number)).toEqual([
      372, 373, 374, 375, 376, 377, 378, 379, 380,
    ]);
  });

  it("leaves an unstacked set in the order it came", () => {
    const prs = [pr(9, "OPEN", "a"), pr(3, "OPEN", "b"), pr(5, "OPEN", "c")];
    expect(stackOrder(prs).map((p) => p.number)).toEqual([9, 3, 5]);
  });

  it("keeps PRs caught in a base cycle instead of dropping them", () => {
    const prs = [pr(1, "OPEN", "x", "y"), pr(2, "OPEN", "y", "x")];
    expect(stackOrder(prs).map((p) => p.number).sort()).toEqual([1, 2]);
  });
});

