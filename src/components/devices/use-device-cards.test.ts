import { describe, expect, it } from "vitest";

import { host, status, syncRow as row } from "./host-fixtures.test-utils";
import {
  buildDeviceCards,
  cardRowCount,
  describeStatus,
  groupRowsByProject,
  type DeviceCard,
  type DeviceRow,
} from "./use-device-cards";

const NOW = Date.parse("2026-08-27T18:00:00Z");

function titles(card: DeviceCard): string[] {
  return card.projects.flatMap((p) => p.rows.map((r) => r.sync.title));
}

describe("buildDeviceCards", () => {
  it("gives every configured host a card and assigns its rows by server id", () => {
    const cards = buildDeviceCards({
      hosts: [host(1, "pandora"), host(2, "zeus")],
      statuses: {},
      syncRows: [
        row({ host_server_id: "srv-zeus", title: "a" }),
        row({ host_server_id: "srv-zeus", title: "b" }),
        row({ host_server_id: "srv-pandora", title: "c" }),
      ],
      attachedInPlace: [],
      now: NOW,
    });
    expect(cards.map((c) => c.name)).toEqual(["pandora", "zeus"]);
    expect(cardRowCount(cards[0])).toBe(1);
    expect(cardRowCount(cards[1])).toBe(2);
  });

  it("keeps a host that has not synced yet, with no rows", () => {
    const cards = buildDeviceCards({
      hosts: [host(3, "fresh", null)],
      statuses: {},
      syncRows: [row({ host_server_id: null })],
      attachedInPlace: [],
      now: NOW,
    });
    expect(cards[0].name).toBe("fresh");
    expect(cards[0].serverId).toBeNull();
    expect(cardRowCount(cards[0])).toBe(0);
  });

  it("hides a row that is already open in place as an attach-only workspace", () => {
    const cards = buildDeviceCards({
      hosts: [host(2, "zeus")],
      statuses: {},
      syncRows: [
        row({ title: "open-here", origin_path: "/srv/passpage" }),
        row({ title: "still-remote", origin_path: "/srv/other" }),
      ],
      attachedInPlace: [{ hostId: 2, remoteCwd: "/srv/passpage" }],
      now: NOW,
    });
    expect(titles(cards[0])).toEqual(["still-remote"]);
  });

  it("skips rows that still map to a local workspace", () => {
    const cards = buildDeviceCards({
      hosts: [host(2, "zeus")],
      statuses: {},
      syncRows: [row({ workspace_id: "ws-local" }), row({ title: "remote" })],
      attachedInPlace: [],
      now: NOW,
    });
    expect(titles(cards[0])).toEqual(["remote"]);
  });

  it("carries the status tone, disk and Remote Control facts onto the card", () => {
    const cards = buildDeviceCards({
      hosts: [host(2, "zeus")],
      statuses: {
        2: status(2, {
          reachable: true,
          disk_bytes: 4_100_000_000,
          remote_control_serving: true,
        }),
      },
      syncRows: [
        row({ project_uid: "uid-a", project_path: "/p/a" }),
        row({ project_uid: "uid-a", project_path: "/p/a" }),
        row({ project_uid: "uid-b", project_path: "/p/b" }),
      ],
      attachedInPlace: [],
      now: NOW,
    });
    expect(cards[0].projects).toHaveLength(2);
    expect(cards[0].tone).toBe("online");
    expect(cards[0].statusLabel).toBe("online");
    expect(cards[0].remoteControlServing).toBe(true);
    expect(cards[0].diskBytes).toBe(4_100_000_000);
  });

  it("names where a diverged branch's other copies live, per row", () => {
    const branch = { git_branch: "feat", project_remote: "github.com/deus/passpage" };
    const cards = buildDeviceCards({
      hosts: [host(1, "pandora"), host(2, "zeus")],
      statuses: {},
      syncRows: [
        row({ id: 1, ...branch, git_head_sha: "aaa", host_server_id: "srv-zeus" }),
        row({ id: 2, ...branch, git_head_sha: "bbb", host_server_id: "srv-pandora" }),
        // The local copy is not a card row, but it still counts as a fork.
        row({ id: 3, ...branch, git_head_sha: "ccc", host_server_id: null, workspace_id: "local" }),
        row({ id: 4, title: "clean", host_server_id: "srv-zeus" }),
      ],
      attachedInPlace: [],
      now: NOW,
    });
    const byTitle = (card: DeviceCard) =>
      Object.fromEntries(
        card.projects.flatMap((p) => p.rows.map((r) => [r.sync.title, r.divergedLabel])),
      );
    expect(byTitle(cards[0])).toEqual({ "ws-2": "zeus + 1 more" });
    expect(byTitle(cards[1])).toEqual({ "ws-1": "pandora + 1 more", clean: null });
  });

  it("trails rows from an unknown host as an 'Another device' card", () => {
    const cards = buildDeviceCards({
      hosts: [host(2, "zeus")],
      statuses: {},
      syncRows: [row({ host_server_id: "srv-ghost", title: "orphan" })],
      attachedInPlace: [],
      now: NOW,
    });
    expect(cards).toHaveLength(2);
    expect(cards[1].host).toBeNull();
    expect(cards[1].name).toBe("Another device");
    expect(cards[1].tone).toBe("offline");
    expect(cards[1].statusLabel).toBe("not configured on this device");
    expect(cardRowCount(cards[1])).toBe(1);
  });
});

describe("describeStatus", () => {
  it("reads unprobed hosts as still checking", () => {
    expect(describeStatus(null, NOW)).toEqual({
      tone: "checking",
      label: "checking…",
      detail: null,
    });
    expect(describeStatus(status(1, { probed: false }), NOW).tone).toBe("checking");
  });

  it("labels online, degraded, last-seen, and never-reached hosts", () => {
    expect(describeStatus(status(1, { reachable: true }), NOW)).toEqual({
      tone: "online",
      label: "online",
      detail: null,
    });
    expect(
      describeStatus(
        status(1, { reachable: true, last_error: "codemux-remote is not installed" }),
        NOW,
      ),
    ).toEqual({
      tone: "attention",
      label: "needs attention",
      detail: "codemux-remote is not installed",
    });
    expect(
      describeStatus(
        status(1, { last_seen_at: "2026-08-25T18:00:00Z", last_error: "timed out" }),
        NOW,
      ),
    ).toEqual({
      tone: "offline",
      label: "unreachable · last seen 2d ago",
      detail: "timed out",
    });
    expect(describeStatus(status(1), NOW).label).toBe("never reached");
  });
});

describe("groupRowsByProject", () => {
  const plain = (partial: Parameters<typeof row>[0]): DeviceRow => ({
    sync: row(partial),
    divergedLabel: null,
  });

  it("clusters by project_uid, floats the root, and sorts clusters by name", () => {
    const projects = groupRowsByProject([
      plain({ title: "z-wt", project_uid: "uid-z", project_path: "/p/zeta", workspace_kind: "worktree" }),
      plain({ title: "z-root", project_uid: "uid-z", project_path: "/p/zeta", workspace_kind: "main" }),
      plain({ title: "a-root", project_uid: "uid-a", project_path: "/p/alpha", workspace_kind: "main" }),
    ]);
    expect(projects.map((p) => p.name)).toEqual(["alpha", "zeta"]);
    expect(projects[1].rows.map((r) => r.sync.title)).toEqual(["z-root", "z-wt"]);
    expect(projects[1].projectUid).toBe("uid-z");
  });

  it("falls back to the title when a row has no project path", () => {
    const projects = groupRowsByProject([
      plain({ title: "lonely", project_uid: null, project_path: null }),
    ]);
    expect(projects[0].name).toBe("lonely");
    expect(projects[0].projectUid).toBeNull();
  });
});
