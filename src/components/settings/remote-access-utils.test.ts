import { describe, expect, it } from "vitest";

import type {
  WebRemoteEndpoint,
  WebRemotePairingInfo,
  WebRemoteSessionView,
  WebRemoteStatus,
} from "@/tauri/types";

import {
  approvedSessions,
  composePairUrl,
  connectedSessionCount,
  describeDevice,
  endpointIsSecure,
  endpointSecurityHint,
  formatCountdown,
  groupEndpoints,
  msUntil,
  newlyPendingSessionIds,
  pendingSessions,
  pickPrimaryEndpoint,
  relativeTime,
  validatePort,
} from "./remote-access-utils";

function ep(partial: Partial<WebRemoteEndpoint>): WebRemoteEndpoint {
  return {
    kind: "lan",
    group: "local_network",
    host: "192.168.1.5",
    port: 4377,
    url: "http://192.168.1.5:4377",
    secure: false,
    recommended: false,
    label: "",
    ...partial,
  };
}

function sess(partial: Partial<WebRemoteSessionView>): WebRemoteSessionView {
  return {
    id: "s1",
    name: null,
    user_agent: null,
    created_at: "2026-07-04T00:00:00Z",
    last_seen_at: null,
    approved: true,
    connected: false,
    ...partial,
  };
}

function status(sessions: WebRemoteSessionView[]): WebRemoteStatus {
  return {
    enabled: true,
    running: true,
    port: 4377,
    require_approval: true,
    active_connections: 0,
    connected_sessions: sessions.filter((s) => s.connected).length,
    sessions,
  };
}

describe("endpoint security", () => {
  it("loopback (secure flag) is secure", () => {
    const e = ep({ kind: "loopback", host: "127.0.0.1", url: "http://127.0.0.1:4377", secure: true });
    expect(endpointIsSecure(e)).toBe(true);
    expect(endpointSecurityHint(e).secure).toBe(true);
    expect(endpointSecurityHint(e).badge).toBe("Secure");
  });

  it("https url is secure even when the backend secure flag is false", () => {
    const e = ep({ kind: "magicdns", host: "box.ts.net", url: "https://box.ts.net:4377", secure: false });
    expect(endpointIsSecure(e)).toBe(true);
    expect(endpointSecurityHint(e).badge).toBe("Secure");
  });

  it("plain-HTTP LAN is limited, with the clipboard/notifications warning", () => {
    const e = ep({ kind: "lan", url: "http://192.168.1.5:4377", secure: false });
    expect(endpointIsSecure(e)).toBe(false);
    const hint = endpointSecurityHint(e);
    expect(hint.secure).toBe(false);
    expect(hint.badge).toBe("Limited");
    expect(hint.detail).toMatch(/clipboard/i);
    expect(hint.detail).toMatch(/notifications/i);
  });
});

describe("pairing URL composition", () => {
  const pairing: WebRemotePairingInfo = {
    url_path: "/#pair=abc123",
    token: "abc123",
    expires_at: "2026-07-04T00:10:00Z",
  };

  it("concatenates endpoint url + relative pair path", () => {
    expect(composePairUrl(ep({ url: "http://192.168.1.5:4377" }), pairing)).toBe(
      "http://192.168.1.5:4377/#pair=abc123",
    );
  });

  it("prefers a networked endpoint over loopback for the QR target", () => {
    const eps = [
      ep({ kind: "loopback", group: "this_device", host: "127.0.0.1" }),
      ep({ kind: "lan", host: "192.168.1.5" }),
    ];
    expect(pickPrimaryEndpoint(eps)?.host).toBe("192.168.1.5");
  });

  it("prefers the recommended endpoint over any other networked one", () => {
    const eps = [
      ep({ kind: "loopback", group: "this_device", host: "127.0.0.1" }),
      ep({ kind: "lan", host: "192.168.1.5" }),
      ep({
        kind: "magicdns",
        group: "tailscale",
        host: "box.ts.net",
        url: "https://box.ts.net:4377",
        recommended: true,
      }),
    ];
    expect(pickPrimaryEndpoint(eps)?.host).toBe("box.ts.net");
  });

  it("falls back to loopback when it is the only endpoint", () => {
    expect(
      pickPrimaryEndpoint([
        ep({ kind: "loopback", group: "this_device", host: "127.0.0.1" }),
      ])?.host,
    ).toBe("127.0.0.1");
    expect(pickPrimaryEndpoint([])).toBeNull();
  });
});

describe("groupEndpoints", () => {
  it("buckets endpoints into ordered groups and drops empty ones", () => {
    const eps = [
      ep({ kind: "loopback", group: "this_device", host: "127.0.0.1" }),
      ep({ kind: "lan", group: "local_network", host: "192.168.1.5" }),
      ep({ kind: "tailnet", group: "tailscale", host: "100.64.0.1" }),
      ep({ kind: "lan", group: "other", host: "fd00::5", url: "http://[fd00::5]:4377" }),
    ];
    const groups = groupEndpoints(eps);
    // Ordered this_device → local_network → tailscale → other; no empty groups.
    expect(groups.map((g) => g.id)).toEqual([
      "this_device",
      "local_network",
      "tailscale",
      "other",
    ]);
    // Only the catch-all group is collapsible.
    expect(groups.find((g) => g.id === "other")?.collapsible).toBe(true);
    expect(groups.find((g) => g.id === "this_device")?.collapsible).toBe(false);
  });

  it("omits a group with no endpoints (e.g. no Tailscale)", () => {
    const groups = groupEndpoints([
      ep({ kind: "loopback", group: "this_device", host: "127.0.0.1" }),
      ep({ kind: "lan", group: "local_network", host: "192.168.1.5" }),
    ]);
    expect(groups.map((g) => g.id)).toEqual(["this_device", "local_network"]);
  });

  it("degrades an unknown group value into 'other'", () => {
    const groups = groupEndpoints([
      ep({ kind: "lan", group: "made_up", host: "10.0.0.9" }),
    ]);
    expect(groups.map((g) => g.id)).toEqual(["other"]);
    expect(groups[0].endpoints[0].host).toBe("10.0.0.9");
  });
});

describe("session partitioning", () => {
  const sessions = [
    sess({ id: "a", approved: true, connected: true }),
    sess({ id: "b", approved: true, connected: false }),
    sess({ id: "c", approved: false }),
  ];
  const st = status(sessions);

  it("splits pending vs approved", () => {
    expect(pendingSessions(st).map((s) => s.id)).toEqual(["c"]);
    expect(approvedSessions(st).map((s) => s.id)).toEqual(["a", "b"]);
  });

  it("counts only approved + connected devices", () => {
    expect(connectedSessionCount(st)).toBe(1);
  });

  it("is null-safe", () => {
    expect(pendingSessions(null)).toEqual([]);
    expect(approvedSessions(null)).toEqual([]);
    expect(connectedSessionCount(null)).toBe(0);
  });
});

describe("newlyPendingSessionIds", () => {
  it("flags a pending device that was absent before", () => {
    const prev = [sess({ id: "a", approved: true })];
    const next = [
      sess({ id: "a", approved: true }),
      sess({ id: "b", approved: false }),
    ];
    expect(newlyPendingSessionIds(prev, next)).toEqual(["b"]);
  });

  it("does not re-flag a device that was already pending", () => {
    const prev = [sess({ id: "b", approved: false })];
    const next = [sess({ id: "b", approved: false })];
    expect(newlyPendingSessionIds(prev, next)).toEqual([]);
  });

  it("does not flag a device that transitioned to approved", () => {
    const prev = [sess({ id: "b", approved: false })];
    const next = [sess({ id: "b", approved: true })];
    expect(newlyPendingSessionIds(prev, next)).toEqual([]);
  });
});

describe("describeDevice", () => {
  it("uses the reported name and derives platform + kind from the UA", () => {
    const d = describeDevice(
      "Sam's iPhone",
      "Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 Version/17.4 Mobile/15E148 Safari/604.1",
    );
    expect(d.title).toBe("Sam's iPhone");
    expect(d.kind).toBe("phone");
    expect(d.platform).toContain("iOS");
    expect(d.platform).toContain("Safari");
  });

  it("derives a title when no name was reported", () => {
    const d = describeDevice(
      null,
      "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/124.0.0.0 Safari/537.36",
    );
    expect(d.title).toBe("Linux device");
    expect(d.kind).toBe("desktop");
    expect(d.platform).toContain("Chrome");
  });

  it("degrades gracefully for an unknown UA", () => {
    const d = describeDevice(null, null);
    expect(d.title).toBe("Unknown device");
    expect(d.kind).toBe("unknown");
    expect(d.platform).toBe("");
  });

  it("distinguishes macOS + Chrome from Edge", () => {
    expect(
      describeDevice(null, "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/124 Safari/537.36").platform,
    ).toBe("macOS · Chrome");
    expect(
      describeDevice(null, "Mozilla/5.0 (Windows NT 10.0) Chrome/124 Safari/537.36 Edg/124").platform,
    ).toBe("Windows · Edge");
  });
});

describe("time helpers", () => {
  it("formats a mm:ss countdown and clamps at zero", () => {
    expect(formatCountdown(600_000)).toBe("10:00");
    expect(formatCountdown(65_000)).toBe("1:05");
    expect(formatCountdown(9_000)).toBe("0:09");
    expect(formatCountdown(-5)).toBe("0:00");
  });

  it("computes ms-until with a fixed now and clamps negatives", () => {
    const now = Date.parse("2026-07-04T00:00:00Z");
    expect(msUntil("2026-07-04T00:00:30Z", now)).toBe(30_000);
    expect(msUntil("2026-07-03T23:59:00Z", now)).toBe(0);
    expect(msUntil("not-a-date", now)).toBe(0);
  });

  it("renders compact relative time", () => {
    const now = Date.parse("2026-07-04T12:00:00Z");
    expect(relativeTime(null, now)).toBe("never");
    expect(relativeTime("2026-07-04T11:59:50Z", now)).toBe("just now");
    expect(relativeTime("2026-07-04T11:57:00Z", now)).toBe("3m ago");
    expect(relativeTime("2026-07-04T10:00:00Z", now)).toBe("2h ago");
    expect(relativeTime("2026-07-01T12:00:00Z", now)).toBe("3d ago");
  });

  it("treats SQLite datetime('now') strings (no timezone designator) as UTC", () => {
    const now = Date.parse("2026-07-04T12:00:00Z");
    // Same instants as above, in SQLite's "YYYY-MM-DD HH:MM:SS" UTC format —
    // must not be skewed by the machine's local UTC offset.
    expect(relativeTime("2026-07-04 11:59:50", now)).toBe("just now");
    expect(relativeTime("2026-07-04 11:57:00", now)).toBe("3m ago");
    expect(relativeTime("2026-07-04 10:00:00", now)).toBe("2h ago");
  });
});

describe("validatePort", () => {
  it("accepts in-range ports", () => {
    expect(validatePort("4377")).toEqual({ valid: true, value: 4377, error: null });
    expect(validatePort(" 8080 ")).toEqual({ valid: true, value: 8080, error: null });
  });

  it("rejects non-numeric and out-of-range values", () => {
    expect(validatePort("abc").valid).toBe(false);
    expect(validatePort("80").valid).toBe(false);
    expect(validatePort("70000").valid).toBe(false);
    expect(validatePort("").valid).toBe(false);
  });
});
