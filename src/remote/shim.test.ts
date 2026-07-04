import { describe, it, expect, afterEach } from "vitest";
import { Channel } from "@tauri-apps/api/core";
import { installShim, type ShimHandle } from "./shim";
import { FakeWebSocket, createFakeFetch, buildBinaryFrame, flushMicrotasks } from "./testing";

interface Internals {
  invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
  transformCallback: (fn: (payload: unknown) => void, once?: boolean) => number;
  unregisterCallback: (id: number) => void;
  convertFileSrc: (path: string) => string;
}

interface EventInternals {
  unregisterListener: (event: string, eventId: number) => void;
}

function internals(): Internals {
  return (window as unknown as { __TAURI_INTERNALS__: Internals })
    .__TAURI_INTERNALS__;
}
function eventInternals(): EventInternals {
  return (
    window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: EventInternals }
  ).__TAURI_EVENT_PLUGIN_INTERNALS__;
}

const handles: ShimHandle[] = [];

async function setup(): Promise<{ ws: FakeWebSocket }> {
  const fetch = createFakeFetch();
  const handle = installShim({
    baseUrl: "http://desk.local:4377",
    getToken: () => "session-token",
    appVersion: "9.9.9",
    deps: {
      fetchImpl: fetch.fetchImpl,
      wsFactory: (url) => new FakeWebSocket(url),
    },
  });
  handles.push(handle);
  const p = handle.transport.connect();
  await flushMicrotasks();
  const ws = FakeWebSocket.instances[FakeWebSocket.instances.length - 1];
  ws.open();
  await p;
  return { ws };
}

afterEach(() => {
  for (const h of handles) h.transport.close();
  handles.length = 0;
  FakeWebSocket.reset();
});

describe("shim install", () => {
  it("sets __CODEMUX_REMOTE__ and the internals surface", async () => {
    await setup();
    expect((window as unknown as { __CODEMUX_REMOTE__: boolean }).__CODEMUX_REMOTE__).toBe(true);
    const i = internals();
    expect(typeof i.invoke).toBe("function");
    expect(typeof i.transformCallback).toBe("function");
    expect(typeof i.unregisterCallback).toBe("function");
    expect(typeof i.convertFileSrc).toBe("function");
  });

  it("maps convertFileSrc to the authed asset route on the server origin", async () => {
    await setup();
    expect(internals().convertFileSrc("/home/u/x.png")).toBe(
      "http://desk.local:4377/api/assets?path=%2Fhome%2Fu%2Fx.png",
    );
  });
});

describe("invoke routing", () => {
  it("forwards real commands over the wire and resolves on ok", async () => {
    const { ws } = await setup();
    const p = internals().invoke("get_app_state", {});
    const sent = ws.lastSent();
    expect(sent).toMatchObject({ t: "invoke", cmd: "get_app_state" });
    ws.recvText({ t: "ok", id: sent.id, data: { workspaces: [] } });
    await expect(p).resolves.toEqual({ workspaces: [] });
  });

  it("rejects a forwarded invoke on an err frame", async () => {
    const { ws } = await setup();
    const p = internals().invoke("do_thing", {});
    const id = ws.lastSent().id;
    ws.recvText({ t: "err", id, error: "boom" });
    await expect(p).rejects.toBe("boom");
  });

  it("answers plugin routes locally without touching the socket", async () => {
    const { ws } = await setup();
    await expect(internals().invoke("plugin:app|version")).resolves.toBe("9.9.9");
    await expect(internals().invoke("plugin:updater|check")).resolves.toBeNull();
    await expect(internals().invoke("plugin:dialog|open")).resolves.toBeNull();
    await expect(internals().invoke("plugin:process|exit")).resolves.toBeNull();
    await expect(internals().invoke("plugin:window|is_maximized")).resolves.toBe(false);
    await expect(internals().invoke("plugin:window|scale_factor")).resolves.toBe(1);
    // None of these produced an invoke frame.
    expect(ws.parsedSent().some((m) => m.t === "invoke")).toBe(false);
  });
});

describe("event listen/unlisten refcounting", () => {
  it("sends one listen for many subscribers and one unlisten at the last", async () => {
    const { ws } = await setup();
    const i = internals();
    const cb1 = i.transformCallback(() => {});
    const cb2 = i.transformCallback(() => {});
    const eid1 = (await i.invoke("plugin:event|listen", {
      event: "evt",
      handler: cb1,
    })) as number;
    const eid2 = (await i.invoke("plugin:event|listen", {
      event: "evt",
      handler: cb2,
    })) as number;

    const listens = ws
      .parsedSent()
      .filter((m) => m.t === "listen" && m.event === "evt");
    expect(listens).toHaveLength(1);

    // Real @tauri-apps unlisten signals twice (event-internals + invoke);
    // the refcount must move only once. First listener → no unlisten yet.
    eventInternals().unregisterListener("evt", eid1);
    await i.invoke("plugin:event|unlisten", { event: "evt", eventId: eid1 });
    expect(
      ws.parsedSent().filter((m) => m.t === "unlisten" && m.event === "evt"),
    ).toHaveLength(0);

    // Last listener → exactly one unlisten frame.
    eventInternals().unregisterListener("evt", eid2);
    await i.invoke("plugin:event|unlisten", { event: "evt", eventId: eid2 });
    expect(
      ws.parsedSent().filter((m) => m.t === "unlisten" && m.event === "evt"),
    ).toHaveLength(1);
  });

  it("delivers event frames to registered listeners with the Tauri shape", async () => {
    const { ws } = await setup();
    const i = internals();
    const received: Array<{ event: string; id: number; payload: unknown }> = [];
    const cb = i.transformCallback((p) =>
      received.push(p as { event: string; id: number; payload: unknown }),
    );
    const eid = (await i.invoke("plugin:event|listen", {
      event: "app-state-changed",
      handler: cb,
    })) as number;

    ws.recvText({ t: "event", event: "app-state-changed", payload: { n: 5 } });

    expect(received).toHaveLength(1);
    expect(received[0]).toMatchObject({
      event: "app-state-changed",
      id: eid,
      payload: { n: 5 },
    });
  });
});

describe("channel machinery (real @tauri-apps Channel)", () => {
  it("orders frames and decodes binary bodies through onmessage", async () => {
    const { ws } = await setup();
    const messages: unknown[] = [];
    const channel = new Channel<unknown>();
    channel.onmessage = (m) => messages.push(m);

    // The channel serializes into invoke args as "__CHANNEL__:<id>".
    const p = internals().invoke("attach_pty_output", {
      sessionId: "s1",
      channel,
    });
    const sent = ws.lastSent();
    expect((sent.args as Record<string, unknown>).channel).toBe(
      `__CHANNEL__:${channel.id}`,
    );
    ws.recvText({ t: "ok", id: sent.id, data: 0 });
    await p;

    // Deliver OUT OF ORDER: idx 1 (binary) before idx 0 (JSON). The
    // Channel must buffer idx 1 until idx 0 arrives, then flush in order.
    ws.recvBinary(buildBinaryFrame(channel.id, 1, new Uint8Array([65, 66])));
    expect(messages).toHaveLength(0);

    ws.recvText({ t: "chan", ch: channel.id, idx: 0, data: "first" });

    expect(messages).toHaveLength(2);
    expect(messages[0]).toBe("first");
    expect(messages[1]).toBeInstanceOf(Uint8Array);
    expect(Array.from(messages[1] as Uint8Array)).toEqual([65, 66]);
  });
});
