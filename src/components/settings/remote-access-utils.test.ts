import { describe, expect, it } from "vitest";

import type {
  WebRemoteEndpoint,
  WebRemotePairingInfo,
  WebRemoteSessionView,
  WebRemoteStatus,
} from "@/tauri/types";

import {
  approvedSessions,
  BIND_SCOPE_OPTIONS,
  bindScopeLabel,
  bindScopeOf,
  classifyOriginHost,
  composePairUrl,
  connectedSessionCount,
  describeDevice,
  endpointIsSecure,
  endpointSecurityHint,
  formatCountdown,
  groupEndpoints,
  isRebindDisconnectError,
  msUntil,
  newlyPendingSessionIds,
  originHostSurvivesScope,
  originSurvivesScope,
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

  it("a public address is called out as internet-reachable", () => {
    const e = ep({
      kind: "public",
      group: "public_internet",
      host: "203.0.114.9",
      url: "http://203.0.114.9:4377",
      secure: false,
    });
    const hint = endpointSecurityHint(e);
    expect(hint.secure).toBe(false);
    expect(hint.badge).toBe("Public");
    expect(hint.detail).toMatch(/internet/i);
    // Still an insecure origin, so the browser-API caveat stays.
    expect(hint.detail).toMatch(/clipboard/i);
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

  it("gives public addresses their own always-visible section, after Tailscale", () => {
    const eps = [
      ep({ kind: "loopback", group: "this_device", host: "127.0.0.1" }),
      ep({ kind: "tailnet", group: "tailscale", host: "100.64.0.1" }),
      ep({
        kind: "public",
        group: "public_internet",
        host: "203.0.114.9",
        url: "http://203.0.114.9:4377",
      }),
      ep({ kind: "lan", group: "other", host: "fd00::5", url: "http://[fd00::5]:4377" }),
    ];
    const groups = groupEndpoints(eps);
    expect(groups.map((g) => g.id)).toEqual([
      "this_device",
      "tailscale",
      "public_internet",
      "other",
    ]);
    // Never buried behind a disclosure — it may be the only way in.
    expect(groups.find((g) => g.id === "public_internet")?.collapsible).toBe(false);
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

describe("bind scope", () => {
  it("offers exactly the three scopes in a stable order", () => {
    expect(BIND_SCOPE_OPTIONS.map((o) => o.value)).toEqual([
      "all",
      "tailscale",
      "loopback",
    ]);
    // Every option carries a label and a one-line explanation.
    for (const o of BIND_SCOPE_OPTIONS) {
      expect(o.label.length).toBeGreaterThan(0);
      expect(o.detail.length).toBeGreaterThan(0);
    }
  });

  it("defaults a status with no bind_scope to 'all'", () => {
    // A config persisted before the field existed omits it on the wire.
    expect(bindScopeOf(status([]))).toBe("all");
    expect(bindScopeOf(null)).toBe("all");
  });

  it("reflects an explicit bind_scope", () => {
    expect(bindScopeOf({ ...status([]), bind_scope: "tailscale" })).toBe(
      "tailscale",
    );
    expect(bindScopeOf({ ...status([]), bind_scope: "loopback" })).toBe(
      "loopback",
    );
  });

  it("labels each scope for interpolating into copy", () => {
    expect(bindScopeLabel("all")).toBe("All networks");
    expect(bindScopeLabel("tailscale")).toBe("Tailscale only");
    expect(bindScopeLabel("loopback")).toBe("This device only");
  });
});

describe("classifyOriginHost", () => {
  it("classifies loopback origins (localhost, 127.x, ::1, brackets)", () => {
    expect(classifyOriginHost("localhost")).toBe("loopback");
    expect(classifyOriginHost("127.0.0.1")).toBe("loopback");
    expect(classifyOriginHost("127.5.6.7")).toBe("loopback");
    expect(classifyOriginHost("::1")).toBe("loopback");
    expect(classifyOriginHost("[::1]")).toBe("loopback");
  });

  it("classifies Tailscale origins (MagicDNS, CGNAT IPv4, IPv6 ULA)", () => {
    expect(classifyOriginHost("mac-studio.tail9c2f.ts.net")).toBe("tailnet");
    expect(classifyOriginHost("100.64.0.1")).toBe("tailnet");
    expect(classifyOriginHost("100.127.255.255")).toBe("tailnet");
    expect(classifyOriginHost("fd7a:115c:a1e0::42")).toBe("tailnet");
  });

  it("treats LAN IPs and anything unrecognised as lan (conservative)", () => {
    expect(classifyOriginHost("192.168.1.42")).toBe("lan");
    expect(classifyOriginHost("10.0.0.9")).toBe("lan");
    // 100.x outside the CGNAT /10 is public space, not tailnet.
    expect(classifyOriginHost("100.63.255.255")).toBe("lan");
    expect(classifyOriginHost("100.128.0.0")).toBe("lan");
    expect(classifyOriginHost("box.example.com")).toBe("lan");
    expect(classifyOriginHost("")).toBe("lan");
  });
});

describe("originSurvivesScope — rebind cutoff prediction", () => {
  it("every origin survives the widest scope (all)", () => {
    expect(originSurvivesScope("loopback", "all")).toBe(true);
    expect(originSurvivesScope("tailnet", "all")).toBe(true);
    expect(originSurvivesScope("lan", "all")).toBe(true);
  });

  it("tailscale scope keeps loopback + tailnet, cuts off LAN", () => {
    expect(originSurvivesScope("loopback", "tailscale")).toBe(true);
    expect(originSurvivesScope("tailnet", "tailscale")).toBe(true);
    expect(originSurvivesScope("lan", "tailscale")).toBe(false);
  });

  it("loopback scope keeps only loopback", () => {
    expect(originSurvivesScope("loopback", "loopback")).toBe(true);
    expect(originSurvivesScope("tailnet", "loopback")).toBe(false);
    expect(originSurvivesScope("lan", "loopback")).toBe(false);
  });

  it("composes host + scope end to end", () => {
    // A loopback browser (the e2e/pairing case) survives every scope — no cutoff.
    expect(originHostSurvivesScope("127.0.0.1", "loopback")).toBe(true);
    expect(originHostSurvivesScope("127.0.0.1", "tailscale")).toBe(true);
    // A LAN browser is cut off by both narrower scopes.
    expect(originHostSurvivesScope("192.168.1.42", "tailscale")).toBe(false);
    expect(originHostSurvivesScope("192.168.1.42", "loopback")).toBe(false);
    // A tailnet browser survives tailscale but not loopback-only.
    expect(originHostSurvivesScope("box.tail1234.ts.net", "tailscale")).toBe(true);
    expect(originHostSurvivesScope("box.tail1234.ts.net", "loopback")).toBe(false);
  });
});

describe("isRebindDisconnectError — expected, not a failure", () => {
  it("classifies the transport's disconnect signals as expected", () => {
    // What the shim's reconnecting transport rejects an in-flight invoke with
    // when a rebind (port/scope change) drops the socket before it can answer.
    expect(isRebindDisconnectError(new Error("web-remote: connection lost"))).toBe(
      true,
    );
    expect(
      isRebindDisconnectError(
        new Error('web-remote: not connected (cannot invoke "web_remote_set_config")'),
      ),
    ).toBe(true);
    // A bare string reason (some transports reject with the raw message).
    expect(isRebindDisconnectError("web-remote: connection lost")).toBe(true);
  });

  it("does NOT swallow a genuine backend error", () => {
    // e.g. Tailscale scope with no tailnet address — the server rejects and
    // keeps the old scope; that must still surface as an error.
    expect(
      isRebindDisconnectError(
        new Error("No Tailscale address found — connect Tailscale or choose a different access scope"),
      ),
    ).toBe(false);
    expect(isRebindDisconnectError("Unknown access scope: bogus")).toBe(false);
    expect(isRebindDisconnectError(null)).toBe(false);
    expect(isRebindDisconnectError(undefined)).toBe(false);
  });
});
