import { describe, it, expect, vi } from "vitest";
import {
  HostedFlow,
  isHostedOrigin,
  apiBaseUrl,
  type HostedConnectHandlers,
  type HostedFlowDeps,
  type HostedState,
} from "./hosted";
import type { RegisteredDevice } from "./device-registry";

const device = (id: string): RegisteredDevice => ({
  id,
  deviceId: `inst-${id}`,
  nodeId: `node-${id}`,
  name: `Device ${id}`,
  platform: "linux",
  lastSeenAt: null,
});

function track(flow: HostedFlow): { states: HostedState[]; last: () => HostedState } {
  const states: HostedState[] = [flow.getState()];
  flow.subscribe((s) => states.push(s));
  return { states, last: () => states[states.length - 1] };
}

describe("HostedFlow: sign-in → list", () => {
  it("moves to the device list on successful sign-in", async () => {
    const deps: HostedFlowDeps = {
      signIn: vi.fn(async () => [device("a"), device("b")]),
      connect: vi.fn(async () => {}),
    };
    const flow = new HostedFlow(deps);
    const t = track(flow);
    await flow.submitSignIn("me@example.com", "pw");
    expect(deps.signIn).toHaveBeenCalledWith("me@example.com", "pw");
    expect(t.last().phase).toBe("devices");
    expect(t.last().devices.map((d) => d.id)).toEqual(["a", "b"]);
    expect(t.last().busy).toBe(false);
  });

  it("lands on an empty device list (no devices registered)", async () => {
    const flow = new HostedFlow({
      signIn: async () => [],
      connect: async () => {},
    });
    await flow.submitSignIn("me@example.com", "pw");
    expect(flow.getState().phase).toBe("devices");
    expect(flow.getState().devices).toEqual([]);
  });

  it("stays on sign-in and surfaces the error on failure", async () => {
    const flow = new HostedFlow({
      signIn: async () => {
        throw new Error("That email or password is incorrect.");
      },
      connect: async () => {},
    });
    await flow.submitSignIn("me@example.com", "bad");
    expect(flow.getState().phase).toBe("signin");
    expect(flow.getState().error).toMatch(/incorrect/);
    expect(flow.getState().busy).toBe(false);
  });

  it("ignores a second sign-in while one is in flight", async () => {
    let resolve!: (d: RegisteredDevice[]) => void;
    const signIn = vi.fn(() => new Promise<RegisteredDevice[]>((r) => (resolve = r)));
    const flow = new HostedFlow({ signIn, connect: async () => {} });
    const p1 = flow.submitSignIn("a@b.c", "pw");
    const p2 = flow.submitSignIn("a@b.c", "pw"); // ignored (busy)
    resolve([device("a")]);
    await Promise.all([p1, p2]);
    expect(signIn).toHaveBeenCalledTimes(1);
  });
});

describe("HostedFlow: select → connect", () => {
  it("connects and reaches the connected phase", async () => {
    const flow = new HostedFlow({
      signIn: async () => [device("a")],
      connect: async () => {},
    });
    await flow.submitSignIn("me@example.com", "pw");
    await flow.select(device("a"));
    expect(flow.getState().phase).toBe("connected");
    expect(flow.getState().selected?.id).toBe("a");
  });

  it("surfaces waiting-for-approval while pending", async () => {
    let handlers!: HostedConnectHandlers;
    const flow = new HostedFlow({
      signIn: async () => [device("a")],
      connect: (_d, h) =>
        new Promise<void>((resolve) => {
          handlers = h;
          h.onPending();
          // resolve once "approved"
          setTimeout(resolve, 0);
        }),
    });
    await flow.submitSignIn("me@example.com", "pw");
    const p = flow.select(device("a"));
    // synchronously after onPending the state reflects waiting
    expect(flow.getState().connectStatus).toBe("waiting-approval");
    handlers.onConnecting();
    expect(flow.getState().connectStatus).toBe("connecting");
    await p;
    expect(flow.getState().phase).toBe("connected");
  });

  it("reflects offline-retrying via the handler", async () => {
    const flow = new HostedFlow({
      signIn: async () => [device("a")],
      connect: (_d, h) =>
        new Promise<void>((resolve) => {
          h.onOfflineRetry();
          setTimeout(resolve, 0);
        }),
    });
    await flow.submitSignIn("me@example.com", "pw");
    const p = flow.select(device("a"));
    expect(flow.getState().connectStatus).toBe("offline-retrying");
    await p;
  });

  it("returns to sign-in when connect fails with unauthorized", async () => {
    const flow = new HostedFlow({
      signIn: async () => [device("a")],
      connect: async () => {
        throw { reason: "unauthorized", message: "Session expired." };
      },
    });
    await flow.submitSignIn("me@example.com", "pw");
    await flow.select(device("a"));
    expect(flow.getState().phase).toBe("signin");
    expect(flow.getState().error).toMatch(/expired/i);
  });

  it("returns to the device list when connect is rejected", async () => {
    const flow = new HostedFlow({
      signIn: async () => [device("a"), device("b")],
      connect: async () => {
        throw new Error("relay client unavailable");
      },
    });
    await flow.submitSignIn("me@example.com", "pw");
    await flow.select(device("a"));
    expect(flow.getState().phase).toBe("devices");
    expect(flow.getState().error).toMatch(/unavailable/);
    // The device list is preserved so the user can retry.
    expect(flow.getState().devices).toHaveLength(2);
  });
});

describe("isHostedOrigin", () => {
  it("is true on the canonical hosted hostname", () => {
    expect(isHostedOrigin({ hostname: "app.codemux.org", search: "" }, {})).toBe(true);
  });
  it("is true with the ?hosted query flag", () => {
    expect(isHostedOrigin({ hostname: "localhost", search: "?hosted" }, {})).toBe(true);
  });
  it("is true with the build flag", () => {
    expect(
      isHostedOrigin({ hostname: "localhost", search: "" }, { VITE_CODEMUX_HOSTED: "true" }),
    ).toBe(true);
  });
  it("is false for a LAN/desktop origin", () => {
    expect(isHostedOrigin({ hostname: "192.168.1.5", search: "" }, {})).toBe(false);
  });
});

describe("apiBaseUrl", () => {
  it("defaults to the production control plane", () => {
    expect(apiBaseUrl({})).toBe("https://api.codemux.org");
  });
  it("honors a build override and trims a trailing slash", () => {
    expect(apiBaseUrl({ VITE_CODEMUX_API_BASE: "http://localhost:8080/" })).toBe(
      "http://localhost:8080",
    );
  });
});
