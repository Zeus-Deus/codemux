import { describe, it, expect, beforeEach } from "vitest";
import {
  useSidebarDensityStore,
  formatElapsed,
  isRetiredPr,
  type WorkObservation,
} from "./sidebar-density-store";

/** Convenience: drive the store's `observeWork` for one workspace. */
function observe(id: string, obs: WorkObservation) {
  useSidebarDensityStore.getState().observeWork(id, obs);
}

/** Read a workspace's shipped history out of the store. */
function shipped(id: string) {
  return useSidebarDensityStore.getState().workHistory[id]?.shipped ?? [];
}

const merged = (
  prNumber: number,
  issueNumber: number | null,
  title: string | null,
): WorkObservation => ({
  prNumber,
  prState: "merged",
  issueNumber,
  issueTitle: title,
});

beforeEach(() => {
  useSidebarDensityStore.setState({
    statusSince: {},
    settledAt: {},
    lastSeenAt: {},
    workHistory: {},
  });
});

describe("sidebar density store — living-row work history", () => {
  it("promotes a merged PR to shipped only once a *different* issue is linked", () => {
    const id = "ws-1";
    // Baseline: merged PR #128 shipped issue A (#50). Nothing shipped yet —
    // the merge is a pending candidate until the workspace moves on.
    observe(id, merged(128, 50, "Work A"));
    expect(shipped(id)).toHaveLength(0);

    // New work started (issue B) → the baseline retires into history.
    observe(id, { prNumber: 128, prState: "merged", issueNumber: 60, issueTitle: "Work B" });
    expect(shipped(id)).toEqual([
      { prNumber: 128, issueNumber: 50, title: "Work A" },
    ]);
  });

  it("marks the promoted PR as retired (icon falls back to the branch)", () => {
    const id = "ws-1";
    observe(id, merged(128, 50, "Work A"));
    observe(id, merged(999, 60, "Work B")); // issue moved on to B
    expect(isRetiredPr(shipped(id), 128)).toBe(true);
    // The freshly-observed merge (#999) is still a pending candidate, not
    // retired, so its PR icon stays.
    expect(isRetiredPr(shipped(id), 999)).toBe(false);
  });

  it("counts each distinct merged PR — tally increments across ships", () => {
    const id = "ws-1";
    observe(id, merged(128, 50, "Work A")); // baseline A
    observe(id, merged(200, 60, "Work B")); // A retires; B baseline
    observe(id, merged(300, 70, "Work C")); // B retires; C baseline
    expect(shipped(id).map((r) => r.prNumber)).toEqual([128, 200]);
    expect(shipped(id)).toHaveLength(2);
  });

  it("is a no-op when the observed tuple is unchanged (no double-count)", () => {
    const id = "ws-1";
    observe(id, merged(128, 50, "Work A"));
    observe(id, merged(200, 60, "Work B")); // ships #128
    const before = useSidebarDensityStore.getState().workHistory[id];
    // Same tuple again — reference must be identical (no write).
    observe(id, merged(200, 60, "Work B"));
    expect(useSidebarDensityStore.getState().workHistory[id]).toBe(before);
    expect(shipped(id)).toHaveLength(1);
  });

  it("dedups a lingering merged pr_state so it ships at most once", () => {
    const id = "ws-1";
    observe(id, merged(128, 50, "Work A"));
    observe(id, merged(128, 60, "Work B")); // ships #128 (issue A→B)
    // PR #128 still reads merged while a new PR hasn't opened; issue moves to
    // C. #128 already shipped, so it must not be counted again.
    observe(id, merged(128, 70, "Work C"));
    expect(shipped(id)).toHaveLength(1);
  });

  it("baseline-promotion: app starts merged + issue A, issue changes to B → 1 shipped", () => {
    const id = "ws-1";
    // First observation at app start = the baseline (no transition seen yet).
    observe(id, merged(128, 50, "Work A"));
    expect(shipped(id)).toHaveLength(0);
    // Linked issue changes to a different issue → promote the baseline.
    observe(id, merged(128, 99, "Work B"));
    expect(shipped(id)).toEqual([
      { prNumber: 128, issueNumber: 50, title: "Work A" },
    ]);
  });

  it("falls back to the PR number when a merge had no linked issue", () => {
    const id = "ws-1";
    // Merge observed with no linked issue → record carries only prNumber.
    // (lastIssueNumber starts null, so the promotion trigger needs a prior
    // issue; seed one first, then a bare-merge candidate, then move on.)
    observe(id, merged(70, 10, "Seed"));
    observe(id, { prNumber: 128, prState: "merged", issueNumber: null, issueTitle: null });
    observe(id, { prNumber: 128, prState: "merged", issueNumber: 20, issueTitle: "Next" });
    const rec = shipped(id).find((r) => r.prNumber === 128);
    expect(rec).toEqual({ prNumber: 128 });
  });
});

describe("formatElapsed", () => {
  const sec = 1000;
  const min = 60 * sec;
  const hour = 60 * min;
  const day = 24 * hour;

  it("reads in seconds, then minutes, then hours", () => {
    expect(formatElapsed(0)).toBe("0s");
    expect(formatElapsed(-5 * sec)).toBe("0s");
    expect(formatElapsed(45 * sec)).toBe("45s");
    expect(formatElapsed(6 * min)).toBe("6m");
    expect(formatElapsed(59 * min)).toBe("59m");
    expect(formatElapsed(hour)).toBe("1h");
    expect(formatElapsed(hour + 12 * min)).toBe("1h12m");
    expect(formatElapsed(23 * hour + 59 * min)).toBe("23h59m");
  });

  it("switches to days past 24h rather than a three-digit hour count", () => {
    expect(formatElapsed(day)).toBe("1d");
    expect(formatElapsed(day + 30 * min)).toBe("1d");
    expect(formatElapsed(day + 3 * hour)).toBe("1d3h");
    expect(formatElapsed(4 * day + 3 * hour + 40 * min)).toBe("4d3h");
    // The old cap rendered this as "96h".
    expect(formatElapsed(4 * day)).toBe("4d");
  });
});
