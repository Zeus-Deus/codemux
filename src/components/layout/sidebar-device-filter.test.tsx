import { describe, expect, it } from "vitest";

import { applyDeviceFilter } from "./sidebar-device-filter";

// Pure-function tests for the filter helper. The dropdown itself
// relies on Radix portals + the hosts store, which is exercised
// indirectly by the DevicePicker tests; here we just want to lock
// down the filter semantics so future refactors don't quietly
// change which workspaces show up under "This device" vs
// "All devices" vs a specific host.

interface FakeWs {
  workspace_id: string;
  host_id?: number | null;
}

const local1: FakeWs = { workspace_id: "ws-1" };
const local2: FakeWs = { workspace_id: "ws-2", host_id: null };
const remote7a: FakeWs = { workspace_id: "ws-3", host_id: 7 };
const remote7b: FakeWs = { workspace_id: "ws-4", host_id: 7 };
const remote8: FakeWs = { workspace_id: "ws-5", host_id: 8 };
const ALL = [local1, local2, remote7a, remote7b, remote8];

describe("applyDeviceFilter", () => {
  it("'all' returns the list verbatim", () => {
    expect(applyDeviceFilter(ALL, "all")).toEqual(ALL);
  });

  it("'local' keeps undefined and null host_id, drops every remote", () => {
    expect(applyDeviceFilter(ALL, "local")).toEqual([local1, local2]);
  });

  it("a specific host id keeps only workspaces on THAT host", () => {
    expect(applyDeviceFilter(ALL, 7)).toEqual([remote7a, remote7b]);
    expect(applyDeviceFilter(ALL, 8)).toEqual([remote8]);
  });

  it("returns an empty array for a host with no workspaces", () => {
    expect(applyDeviceFilter(ALL, 9999)).toEqual([]);
  });

  it("treats undefined and null host_id as equivalent (both = local)", () => {
    // Belt-and-suspenders: the Rust type is Option<i64> which
    // serializes as null, but consumers sometimes spread workspaces
    // without the field at all. Both must count as "local" — losing
    // one or the other would hide workspaces from "This device."
    const undef: FakeWs = { workspace_id: "u" };
    const nullish: FakeWs = { workspace_id: "n", host_id: null };
    expect(applyDeviceFilter([undef, nullish], "local")).toEqual([
      undef,
      nullish,
    ]);
  });
});
