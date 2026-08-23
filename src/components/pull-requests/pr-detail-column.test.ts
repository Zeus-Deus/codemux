import { describe, it, expect } from "vitest";

import { checksAreSettled } from "./pr-detail-column";
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

describe("checksAreSettled", () => {
  it("is not settled before the first answer arrives", () => {
    // The fast cadence is what gets that answer on screen.
    expect(checksAreSettled(undefined, false, null)).toBe(false);
  });

  it("is not settled while anything is still running", () => {
    expect(checksAreSettled([check(), check({ status: "in_progress", conclusion: null })], false, "pending")).toBe(
      false,
    );
  });

  it("keeps the fast cadence for statuses it does not recognise", () => {
    // `checkState` treats anything outside pass/fail/neutral as running,
    // and this relies on that: an unfamiliar status must never be the
    // reason the column goes quiet.
    for (const status of ["queued", "waiting", "action_required", "pending", ""]) {
      expect(checksAreSettled([check({ status, conclusion: null })], false, null)).toBe(false);
    }
  });

  it("settles once every check has reported", () => {
    expect(
      checksAreSettled(
        [check({ conclusion: "success" }), check({ conclusion: "failure" })],
        false,
        "failing",
      ),
    ).toBe(true);
  });

  it("settles on an empty list only when the host said there is no CI", () => {
    // `"none"` is the host having looked. Anything else is a pull
    // request whose first check has not registered yet — the seconds
    // right after a push, which is precisely when someone is watching.
    expect(checksAreSettled([], false, "none")).toBe(true);
    expect(checksAreSettled([], false, null)).toBe(false);
    expect(checksAreSettled([], false, "pending")).toBe(false);
  });

  it("settles when the checks call itself is failing", () => {
    // Re-asking a host that just said no, twice every five seconds, is
    // the behaviour the budget gate exists to prevent.
    expect(checksAreSettled(undefined, true, null)).toBe(true);
  });
});
