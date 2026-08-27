import { describe, expect, it } from "vitest";

import { syncRow } from "@/components/devices/host-fixtures.test-utils";

import { detectDivergedRows } from "./workspace-divergence";

const branch = { project_remote: "github.com/deus/passpage", git_branch: "feat" };

describe("detectDivergedRows", () => {
  it("ignores rows without a head sha", () => {
    const result = detectDivergedRows([
      syncRow({ id: 1, ...branch, git_head_sha: null, host_server_id: "srv-zeus" }),
      syncRow({ id: 2, ...branch, git_head_sha: "aaa", host_server_id: null }),
    ]);
    expect(result.size).toBe(0);
  });

  it("never flags a branch that lives on one device only", () => {
    const result = detectDivergedRows([
      syncRow({ id: 1, ...branch, git_head_sha: "aaa", host_server_id: "srv-zeus" }),
    ]);
    expect(result.size).toBe(0);
  });

  it("never flags copies at the same commit", () => {
    const result = detectDivergedRows([
      syncRow({ id: 1, ...branch, git_head_sha: "aaa", host_server_id: "srv-zeus" }),
      syncRow({ id: 2, ...branch, git_head_sha: "aaa", host_server_id: null }),
    ]);
    expect(result.size).toBe(0);
  });

  it("flags both copies when the shas differ, each naming the other's host", () => {
    const result = detectDivergedRows([
      syncRow({ id: 1, ...branch, git_head_sha: "aaa", host_server_id: "srv-zeus" }),
      syncRow({ id: 2, ...branch, git_head_sha: "bbb", host_server_id: null }),
      // A different branch at its own sha is unrelated.
      syncRow({ id: 3, ...branch, git_branch: "other", git_head_sha: "ccc" }),
    ]);
    expect(result.size).toBe(2);
    expect(result.get(1)).toEqual({ forks: 2, otherHostServerIds: [null] });
    expect(result.get(2)).toEqual({ forks: 2, otherHostServerIds: ["srv-zeus"] });
  });
});
