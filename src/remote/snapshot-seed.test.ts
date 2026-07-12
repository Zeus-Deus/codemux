import { describe, it, expect, afterEach, vi } from "vitest";
import { fetchSnapshot } from "./snapshot-seed";
import { installShim, type ShimHandle } from "./shim";
import { FakeWebSocket, createFakeFetch, flushMicrotasks } from "./testing";

// ── fetchSnapshot (the HTTP prefetch) ─────────────────────────────────

/** A one-shot fake `fetch` for `/api/snapshot`, recording the request. */
function snapshotFetch(
  response:
    | { ok: true; status?: number; body: unknown }
    | { ok: false; status: number }
    | { throws: true },
): { fetchImpl: typeof fetch; calls: Array<{ url: string; init: RequestInit }> } {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  const impl = async (input: unknown, init?: RequestInit): Promise<unknown> => {
    calls.push({ url: String(input), init: init ?? {} });
    if ("throws" in response) throw new Error("network down");
    if (!response.ok) {
      return { ok: false, status: response.status, json: async () => ({}) };
    }
    return {
      ok: true,
      status: response.status ?? 200,
      json: async () => response.body,
    };
  };
  return { fetchImpl: impl as unknown as typeof fetch, calls };
}

const validEnvelope = {
  api_version: 1,
  app_state: { schema_version: 1, workspaces: [], active_workspace_id: "w1" },
  status: { enabled: true, port: 4377 },
};

describe("fetchSnapshot", () => {
  it("returns app_state from a well-formed envelope and sends bearer + no-store", async () => {
    const f = snapshotFetch({ ok: true, body: validEnvelope });
    const seed = await fetchSnapshot("http://desk.local:4377/", () => "tok", {
      fetchImpl: f.fetchImpl,
    });
    expect(seed).toEqual(validEnvelope.app_state);
    expect(f.calls).toHaveLength(1);
    expect(f.calls[0].url).toBe("http://desk.local:4377/api/snapshot");
    const headers = f.calls[0].init.headers as Record<string, string>;
    expect(headers.Authorization).toBe("Bearer tok");
    expect(f.calls[0].init.cache).toBe("no-store");
    expect(f.calls[0].init.credentials).toBe("include");
  });

  it("returns null on a non-2xx response (e.g. 401)", async () => {
    const f = snapshotFetch({ ok: false, status: 401 });
    expect(
      await fetchSnapshot("http://desk.local:4377", () => "tok", {
        fetchImpl: f.fetchImpl,
      }),
    ).toBeNull();
  });

  it("returns null on a mismatched api_version", async () => {
    const f = snapshotFetch({
      ok: true,
      body: { api_version: 2, app_state: { schema_version: 9 } },
    });
    expect(
      await fetchSnapshot("http://desk.local:4377", () => null, {
        fetchImpl: f.fetchImpl,
      }),
    ).toBeNull();
  });

  it("returns null on a malformed body (missing app_state)", async () => {
    const f = snapshotFetch({ ok: true, body: { api_version: 1 } });
    expect(
      await fetchSnapshot("http://desk.local:4377", () => null, {
        fetchImpl: f.fetchImpl,
      }),
    ).toBeNull();
  });

  it("returns null when fetch throws (network error)", async () => {
    const f = snapshotFetch({ throws: true });
    expect(
      await fetchSnapshot("http://desk.local:4377", () => "tok", {
        fetchImpl: f.fetchImpl,
      }),
    ).toBeNull();
  });
});

// ── Shim seeding (get_app_state) ──────────────────────────────────────

interface Internals {
  invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
  transformCallback: (fn: (payload: unknown) => void, once?: boolean) => number;
}
function internals(): Internals {
  return (window as unknown as { __TAURI_INTERNALS__: Internals }).__TAURI_INTERNALS__;
}

const handles: ShimHandle[] = [];

/** Install the shim with an injected snapshot prefetch and open a socket. */
async function setup(
  fetchSnapshotImpl?: () => Promise<unknown | null>,
): Promise<{ ws: FakeWebSocket; snapshotCalls: () => number }> {
  const ticket = createFakeFetch();
  let snapshotCalls = 0;
  const handle = installShim({
    baseUrl: "http://desk.local:4377",
    getToken: () => "session-token",
    appVersion: "9.9.9",
    deps: {
      fetchImpl: ticket.fetchImpl,
      wsFactory: (url) => new FakeWebSocket(url),
    },
    fetchSnapshot: fetchSnapshotImpl
      ? () => {
          snapshotCalls += 1;
          return fetchSnapshotImpl();
        }
      : undefined,
  });
  handles.push(handle);
  const p = handle.transport.connect();
  await flushMicrotasks();
  const ws = FakeWebSocket.instances[FakeWebSocket.instances.length - 1];
  ws.open();
  await p;
  // Let the prefetch's settle-tracking microtask run.
  await flushMicrotasks();
  return { ws, snapshotCalls: () => snapshotCalls };
}

afterEach(() => {
  for (const h of handles) h.transport.close();
  handles.length = 0;
  FakeWebSocket.reset();
  vi.useRealTimers();
});

const seedState = { schema_version: 1, workspaces: [], active_workspace_id: "seed" };

describe("shim get_app_state seeding", () => {
  it("serves the first get_app_state from the seed without a socket round-trip", async () => {
    const { ws } = await setup(() => Promise.resolve(seedState));

    const result = await internals().invoke("get_app_state", {});
    expect(result).toEqual(seedState);
    // The seeded call never went over the wire.
    expect(
      ws.parsedSent().some((m) => m.t === "invoke" && m.cmd === "get_app_state"),
    ).toBe(false);
  });

  it("serves the seed even while the socket is still pending (WS not open yet)", async () => {
    // Install + settle the prefetch, but do NOT open the socket.
    const ticket = createFakeFetch();
    const handle = installShim({
      baseUrl: "http://desk.local:4377",
      getToken: () => "session-token",
      deps: {
        fetchImpl: ticket.fetchImpl,
        wsFactory: (url) => new FakeWebSocket(url),
      },
      fetchSnapshot: () => Promise.resolve(seedState),
    });
    handles.push(handle);
    void handle.transport.connect();
    await flushMicrotasks();

    // Socket is not open — a raw transport invoke would reject with "not
    // connected", but the seed answers get_app_state anyway.
    await expect(internals().invoke("get_app_state", {})).resolves.toEqual(seedState);
  });

  it("only seeds the FIRST get_app_state; later calls take the WS path", async () => {
    const { ws } = await setup(() => Promise.resolve(seedState));

    await expect(internals().invoke("get_app_state", {})).resolves.toEqual(seedState);

    // Second call forwards over the socket.
    const p = internals().invoke("get_app_state", {});
    const sent = ws.lastSent();
    expect(sent).toMatchObject({ t: "invoke", cmd: "get_app_state" });
    ws.recvText({ t: "ok", id: sent.id, data: { fresh: true } });
    await expect(p).resolves.toEqual({ fresh: true });
  });

  it("discards the seed when a newer app-state already arrived over the WS", async () => {
    const { ws } = await setup(() => Promise.resolve(seedState));

    // A real WS snapshot lands before the app asks — it is authoritative.
    ws.recvText({
      t: "event",
      event: "app-state-changed",
      payload: { schema_version: 1, workspaces: [], active_workspace_id: "ws-fresh" },
    });

    // get_app_state must NOT return the stale seed; it forwards to the socket.
    const p = internals().invoke("get_app_state", {});
    const sent = ws.lastSent();
    expect(sent).toMatchObject({ t: "invoke", cmd: "get_app_state" });
    ws.recvText({ t: "ok", id: sent.id, data: { authoritative: true } });
    await expect(p).resolves.toEqual({ authoritative: true });
  });

  it("falls back cleanly to the WS path when the prefetch fails (null)", async () => {
    const { ws } = await setup(() => Promise.resolve(null));

    const p = internals().invoke("get_app_state", {});
    const sent = ws.lastSent();
    expect(sent).toMatchObject({ t: "invoke", cmd: "get_app_state" });
    ws.recvText({ t: "ok", id: sent.id, data: { fromWs: true } });
    await expect(p).resolves.toEqual({ fromWs: true });
  });

  it("takes the WS path when no fetchSnapshot is provided (seeding disabled)", async () => {
    const { ws } = await setup(undefined);

    const p = internals().invoke("get_app_state", {});
    const sent = ws.lastSent();
    expect(sent).toMatchObject({ t: "invoke", cmd: "get_app_state" });
    ws.recvText({ t: "ok", id: sent.id, data: { fromWs: true } });
    await expect(p).resolves.toEqual({ fromWs: true });
  });
});

describe("shim reconnect re-seeding", () => {
  it("refetches the snapshot on reconnect and pushes it via app-state-changed", async () => {
    vi.useFakeTimers();
    const ticket = createFakeFetch();
    let snapshotCalls = 0;
    const reconnectState = { schema_version: 1, workspaces: [], active_workspace_id: "reconnect" };
    const handle = installShim({
      baseUrl: "http://desk.local:4377",
      getToken: () => "session-token",
      deps: {
        fetchImpl: ticket.fetchImpl,
        wsFactory: (url) => new FakeWebSocket(url),
      },
      fetchSnapshot: () => {
        snapshotCalls += 1;
        return Promise.resolve(reconnectState);
      },
    });
    handles.push(handle);

    const p = handle.transport.connect();
    await flushMicrotasks();
    const ws1 = FakeWebSocket.instances[0];
    ws1.open();
    await p;
    await flushMicrotasks();
    expect(snapshotCalls).toBe(1); // initial prefetch only

    // Subscribe a listener for the app-state event.
    const received: unknown[] = [];
    const cb = internals().transformCallback((ev) =>
      received.push((ev as { payload: unknown }).payload),
    );
    await internals().invoke("plugin:event|listen", {
      event: "app-state-changed",
      handler: cb,
    });

    // Drop, let backoff fire, reopen → this is the reconnect.
    ws1.drop();
    await vi.advanceTimersByTimeAsync(1000);
    await flushMicrotasks();
    const ws2 = FakeWebSocket.instances[1];
    ws2.open();
    await flushMicrotasks();

    expect(snapshotCalls).toBe(2); // refetched on reconnect
    // The reconnect snapshot was pushed into the app via the normal event path.
    expect(received).toContainEqual(reconnectState);
  });

  it("discards the reconnect snapshot if a fresher WS event lands first", async () => {
    vi.useFakeTimers();
    const ticket = createFakeFetch();
    const reconnectState = { schema_version: 1, active_workspace_id: "http-stale" };
    // A deferred prefetch we resolve manually, so a WS event can win the race.
    let resolvePrefetch: (v: unknown) => void = () => {};
    const handle = installShim({
      baseUrl: "http://desk.local:4377",
      getToken: () => "session-token",
      deps: {
        fetchImpl: ticket.fetchImpl,
        wsFactory: (url) => new FakeWebSocket(url),
      },
      fetchSnapshot: () =>
        new Promise((resolve) => {
          resolvePrefetch = resolve;
        }),
    });
    handles.push(handle);

    const p = handle.transport.connect();
    await flushMicrotasks();
    const ws1 = FakeWebSocket.instances[0];
    ws1.open();
    await p;

    const received: unknown[] = [];
    const cb = internals().transformCallback((ev) =>
      received.push((ev as { payload: unknown }).payload),
    );
    await internals().invoke("plugin:event|listen", {
      event: "app-state-changed",
      handler: cb,
    });

    ws1.drop();
    await vi.advanceTimersByTimeAsync(1000);
    await flushMicrotasks();
    const ws2 = FakeWebSocket.instances[1];
    ws2.open();
    await flushMicrotasks();

    // A real, newer WS snapshot arrives BEFORE the reconnect prefetch resolves.
    const fresh = { schema_version: 1, active_workspace_id: "ws-fresh" };
    ws2.recvText({ t: "event", event: "app-state-changed", payload: fresh });
    // Now the stale prefetch resolves — it must be discarded.
    resolvePrefetch(reconnectState);
    await flushMicrotasks();

    expect(received).toContainEqual(fresh);
    expect(received).not.toContainEqual(reconnectState);
  });
});
