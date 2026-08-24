import { describe, it, expect } from "vitest";

import { EMPTY_CHECKS_GRACE_MS, checksAreSettled, newestRefusalAt } from "./pr-detail-column";
import type { CheckInfo } from "@/tauri/types";

function check(over: Partial<CheckInfo> = {}): CheckInfo {
  return {
    name: "build",
    status: "completed",
    conclusion: "success",
    elapsed_time: null,
    detail_url: null,
    started_at: null,
    completed_at: null,
    ...over,
  };
}

/** An open row the stats have not coloured yet. */
const OPEN = { checks: null, state: "OPEN" } as const;

describe("checksAreSettled", () => {
  it("is not settled before the first answer arrives", () => {
    // The fast cadence is what gets that answer on screen.
    expect(checksAreSettled(undefined, false, OPEN, 0)).toBe(false);
    // However long the column has been open, and whatever the row is.
    expect(checksAreSettled(undefined, false, { checks: null, state: "MERGED" }, 600_000)).toBe(
      false,
    );
  });

  it("is not settled while anything is still running", () => {
    expect(
      checksAreSettled(
        [check(), check({ status: "in_progress", conclusion: null })],
        false,
        { checks: "pending", state: "OPEN" },
        0,
      ),
    ).toBe(false);
  });

  it("keeps the fast cadence for statuses it does not recognise", () => {
    // `checkState` treats anything outside pass/fail/neutral as running,
    // and this relies on that: an unfamiliar status must never be the
    // reason the column goes quiet.
    for (const status of ["queued", "waiting", "action_required", "pending", ""]) {
      expect(checksAreSettled([check({ status, conclusion: null })], false, OPEN, 0)).toBe(false);
    }
  });

  it("settles once every check has reported", () => {
    expect(
      checksAreSettled(
        [check({ conclusion: "success" }), check({ conclusion: "failure" })],
        false,
        { checks: "failing", state: "OPEN" },
        0,
      ),
    ).toBe(true);
  });

  it("settles on an empty list when the host said there is no CI", () => {
    // `"none"` is the host having looked.
    expect(checksAreSettled([], false, { checks: "none", state: "OPEN" }, 0)).toBe(true);
    expect(checksAreSettled([], false, { checks: "none" }, 0)).toBe(true);
  });

  it("keeps an open pull request fast while its first check may still register", () => {
    // The seconds right after a push, which is precisely when someone is
    // watching. `null` is nobody having asked; `"pending"` is the row
    // still expecting something.
    expect(checksAreSettled([], false, OPEN, 0)).toBe(false);
    expect(checksAreSettled([], false, { checks: null }, EMPTY_CHECKS_GRACE_MS - 1)).toBe(false);
    expect(checksAreSettled([], false, { checks: "pending", state: "OPEN" }, 0)).toBe(false);
  });

  it("settles an open pull request once the grace has passed with nothing registered", () => {
    // The history list never fills `checks` in, and the stats that do
    // can take a cycle or fail outright — so a row that still says
    // `null` after a minute on the fast clock is a repository with no
    // CI, not one about to report. Left to the row alone, a selected
    // pull request in such a repository polled twice every 2.5s for as
    // long as it was selected.
    expect(checksAreSettled([], false, OPEN, EMPTY_CHECKS_GRACE_MS)).toBe(true);
    expect(checksAreSettled([], false, { checks: "pending" }, EMPTY_CHECKS_GRACE_MS)).toBe(true);
  });

  it("settles a closed or merged pull request on an empty list straight away", () => {
    // Nothing is going to register against a pull request that is done,
    // and the history rows these come from never carry `"none"` — they
    // never asked — so waiting on the row's word would mean waiting
    // forever at the fast cadence.
    for (const state of ["MERGED", "CLOSED", "merged", "closed"]) {
      expect(checksAreSettled([], false, { checks: null, state }, 0)).toBe(true);
      expect(checksAreSettled([], false, { checks: "pending", state }, 0)).toBe(true);
    }
  });

  it("settles when the checks call itself is failing", () => {
    // Re-asking a host that just said no, twice every five seconds, is
    // the behaviour the budget gate exists to prevent.
    expect(checksAreSettled(undefined, true, OPEN, 0)).toBe(true);
  });
});

describe("newestRefusalAt", () => {
  const REFUSAL = "GraphQL: API rate limit already exceeded for user ID 1.";

  it("is nothing when no query has been refused for spending", () => {
    expect(newestRefusalAt([])).toBe(0);
    expect(newestRefusalAt([{ error: null, errorUpdatedAt: 0 }])).toBe(0);
    // A failure that is not a refusal is left to the ordinary error
    // handling: one unreachable repository must not pause the rest.
    expect(
      newestRefusalAt([{ error: "could not resolve host github.com", errorUpdatedAt: 500 }]),
    ).toBe(0);
  });

  it("reports the newest refusal among the queries it is given", () => {
    // Both of the column's live queries can be refused, and either one
    // is enough to raise the gate — but the gate is told once, with the
    // time it can use to tell this refusal from the one before it.
    expect(
      newestRefusalAt([
        { error: REFUSAL, errorUpdatedAt: 100 },
        { error: REFUSAL, errorUpdatedAt: 105 },
        { error: "gh: command not found", errorUpdatedAt: 900 },
      ]),
    ).toBe(105);
  });
});
