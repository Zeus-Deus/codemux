// @vitest-environment node
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { beforeEach, expect, it, vi } from "vitest";
const source = readFileSync(
  new URL("../../public/sw.js", import.meta.url),
  "utf8",
);
let handlers: Record<string, (event: any) => void>;
let worker: any;
const fetch = vi.fn();
beforeEach(() => {
  handlers = {};
  worker = {
    location: { origin: "https://app.codemux.org" },
    addEventListener: (name: string, callback: (event: any) => void) => {
      handlers[name] = callback;
    },
    skipWaiting: vi.fn(),
    registration: { showNotification: vi.fn().mockResolvedValue(undefined) },
    clients: {
      claim: vi.fn(),
      matchAll: vi.fn().mockResolvedValue([]),
      openWindow: vi.fn().mockResolvedValue(undefined),
    },
  };
  fetch.mockReset();
  runInNewContext(source, { self: worker, URL, Response, fetch });
});
it("builds a same-origin host/workspace/pane link from encrypted push data", async () => {
  let done: Promise<unknown> | undefined;
  handlers.push({
    data: {
      json: () => ({
        host: "desktop-1",
        workspace_id: "ws-1",
        pane_id: "p-1",
        title: "Ready",
        url: "https://evil.test",
      }),
    },
    waitUntil: (promise: Promise<unknown>) => {
      done = promise;
    },
  });
  await done;
  expect(worker.registration.showNotification).toHaveBeenCalledWith(
    "Ready",
    expect.objectContaining({
      data: {
        url: "https://app.codemux.org/?device=desktop-1&workspace=ws-1&pane=p-1",
      },
    }),
  );
});
it("does not navigate a notification to another origin", async () => {
  let done: Promise<unknown> | undefined;
  handlers.notificationclick({
    notification: { close: vi.fn(), data: { url: "https://evil.test" } },
    waitUntil: (promise: Promise<unknown>) => {
      done = promise;
    },
  });
  await done;
  expect(worker.clients.openWindow).toHaveBeenCalledWith(
    "https://app.codemux.org/",
  );
});
it("focuses an exact matching destination without creating another window", async () => {
  const focus = vi.fn();
  worker.clients.matchAll.mockResolvedValue([
    { url: "https://app.codemux.org/?device=a&workspace=b", focus },
  ]);
  let done: Promise<unknown> | undefined;
  handlers.notificationclick({
    notification: {
      close: vi.fn(),
      data: { url: "https://app.codemux.org/?device=a&workspace=b" },
    },
    waitUntil: (promise: Promise<unknown>) => {
      done = promise;
    },
  });
  await done;
  expect(focus).toHaveBeenCalledOnce();
  expect(worker.clients.openWindow).not.toHaveBeenCalled();
});
it("shows an offline screen without caching workspace data and leaves API requests alone", async () => {
  fetch.mockRejectedValue(new Error("offline"));
  let response: Promise<Response> | undefined;
  handlers.fetch({
    request: { mode: "navigate" },
    respondWith: (promise: Promise<Response>) => {
      response = promise;
    },
  });
  const result = await response!;
  expect(result.status).toBe(503);
  expect(result.headers.get("Cache-Control")).toBe("no-store");
  expect(await result.text()).toContain("You’re offline");
  const respondWith = vi.fn();
  handlers.fetch({ request: { mode: "cors" }, respondWith });
  expect(respondWith).not.toHaveBeenCalled();
});
