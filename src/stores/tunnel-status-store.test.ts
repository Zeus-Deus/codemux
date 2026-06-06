import { describe, it, expect, beforeEach } from "vitest";

import {
  useTunnelStatusStore,
  tunnelStatusKind,
} from "./tunnel-status-store";

describe("tunnelStatusKind", () => {
  it("shows chrome only for reconnecting and circuit_open", () => {
    expect(tunnelStatusKind(undefined)).toBeNull();
    expect(tunnelStatusKind({ kind: "pending" })).toBeNull();
    expect(tunnelStatusKind({ kind: "connected", ssh_pid: 1 })).toBeNull();
    expect(
      tunnelStatusKind({ kind: "reconnecting", attempt: 2, delay_ms: 1000 }),
    ).toBe("reconnecting");
    expect(
      tunnelStatusKind({ kind: "circuit_open", recent_failures: 5 }),
    ).toBe("lost");
  });
});

describe("useTunnelStatusStore", () => {
  beforeEach(() => {
    useTunnelStatusStore.setState({ byWorkspace: {} });
  });

  it("sets and clears per-workspace status", () => {
    const { setStatus, clear } = useTunnelStatusStore.getState();
    setStatus("ws-1", { kind: "reconnecting", attempt: 1, delay_ms: 1000 });
    expect(useTunnelStatusStore.getState().byWorkspace["ws-1"]).toEqual({
      kind: "reconnecting",
      attempt: 1,
      delay_ms: 1000,
    });
    clear("ws-1");
    expect(useTunnelStatusStore.getState().byWorkspace["ws-1"]).toBeUndefined();
  });

  it("clear is a no-op for an unknown workspace", () => {
    const before = useTunnelStatusStore.getState().byWorkspace;
    useTunnelStatusStore.getState().clear("nope");
    expect(useTunnelStatusStore.getState().byWorkspace).toBe(before);
  });
});
