import { describe, expect, it } from "vitest";

import { host, status } from "@/components/devices/host-fixtures.test-utils";

import { computeDevicesIndicator } from "./devices-attention";

const zeus = host(1, "zeus");
const pandora = host(2, "pandora");
const online = (id: number) => status(id, { reachable: true });
const offline = (id: number) => status(id);

describe("computeDevicesIndicator", () => {
  it("stays quiet while no device has been probed yet", () => {
    expect(
      computeDevicesIndicator({
        hosts: [zeus, pandora],
        statuses: { 1: status(1, { probed: false }) },
        divergedRows: [],
        transferError: null,
      }),
    ).toEqual({ dot: null, tooltip: "Devices" });
  });

  it("says all offline once every probe failed, and counts when some are pending", () => {
    expect(
      computeDevicesIndicator({
        hosts: [zeus, pandora],
        statuses: { 1: offline(1), 2: offline(2) },
        divergedRows: [],
        transferError: null,
      }),
    ).toEqual({ dot: null, tooltip: "Devices — all offline" });

    expect(
      computeDevicesIndicator({
        hosts: [zeus, pandora],
        statuses: { 1: offline(1), 2: status(2, { probed: false }) },
        divergedRows: [],
        transferError: null,
      }),
    ).toEqual({ dot: null, tooltip: "Devices — 1 offline" });
  });

  it("goes green and counts only reachable devices", () => {
    expect(
      computeDevicesIndicator({
        hosts: [zeus, pandora],
        statuses: { 1: online(1), 2: offline(2) },
        divergedRows: [],
        transferError: null,
      }),
    ).toEqual({ dot: "green", tooltip: "Devices — 1 online" });

    expect(
      computeDevicesIndicator({
        hosts: [zeus, pandora],
        statuses: { 1: online(1), 2: online(2) },
        divergedRows: [],
        transferError: null,
      }).tooltip,
    ).toBe("Devices — 2 online");
  });

  it("turns amber for a reachable device that reports a problem", () => {
    expect(
      computeDevicesIndicator({
        hosts: [zeus, pandora],
        statuses: {
          1: online(1),
          2: status(2, { reachable: true, last_error: "codemux-remote is not installed" }),
        },
        divergedRows: [],
        transferError: null,
      }),
    ).toEqual({
      dot: "amber",
      tooltip: "Devices — pandora: codemux-remote is not installed",
    });
  });

  it("lets a diverged branch beat an online device", () => {
    expect(
      computeDevicesIndicator({
        hosts: [zeus],
        statuses: { 1: online(1) },
        divergedRows: [{ title: "bypass-share-limit-owner", hostName: "zeus" }],
        transferError: null,
      }),
    ).toEqual({
      dot: "amber",
      tooltip: "Devices — bypass-share-limit-owner diverged on zeus",
    });
  });

  it("names the branch alone when no device is known, and counts extras", () => {
    expect(
      computeDevicesIndicator({
        hosts: [zeus],
        statuses: {},
        divergedRows: [
          { title: "feat/a", hostName: null },
          { title: "feat/b", hostName: "zeus" },
        ],
        transferError: null,
      }).tooltip,
    ).toBe("Devices — feat/a diverged (+1 more)");
  });

  it("lets a failed transfer beat both divergence and an online device", () => {
    expect(
      computeDevicesIndicator({
        hosts: [zeus],
        statuses: { 1: online(1) },
        divergedRows: [{ title: "main", hostName: "zeus" }],
        transferError: "Pull failed: partpilot",
      }),
    ).toEqual({ dot: "amber", tooltip: "Devices — Pull failed: partpilot" });
  });
});
