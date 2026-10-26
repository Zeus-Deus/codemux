import { describe, expect, it, vi } from "vitest";
import type { PluginInvokeResult } from "./contract";
import {
  acceptPluginViewPort,
  buildPluginViewDocument,
  makePluginViewHost,
  MAX_PLUGIN_CARD_HEIGHT,
  PLUGIN_VIEW_CONTENT_SECURITY_POLICY,
  pluginViewBridgeFeatures,
  type PluginViewContext,
} from "./view-bridge";

const context: PluginViewContext = {
  theme: "dark",
  styles: { "--background": "#000" },
  platform: "desktop",
  placement: "page",
  view: { installationId: "inst-1", viewId: "home" },
  thread: null,
};

function setup(overrides: Partial<Parameters<typeof makePluginViewHost<string>>[0]> = {}) {
  const posted: Record<string, unknown>[] = [];
  const onIntents = vi.fn();
  const invoke = vi.fn(async (): Promise<PluginInvokeResult> => ({ message: "ok", data: { n: 1 } }));
  const host = makePluginViewHost<string>({
    token: "tok",
    post: (message) => posted.push(message as Record<string, unknown>),
    context: () => context,
    captureOrigin: () => "origin",
    invoke,
    onIntents,
    features: ["local-intents"],
    ...overrides,
  });
  const send = (message: Record<string, unknown>) => host.receive({ codemuxPlugin: 1, token: "tok", ...message });
  return { host, posted, onIntents, invoke, send };
}

describe("makePluginViewHost", () => {
  it("ignores messages without this document's token", () => {
    const { host, posted } = setup();
    host.receive({ codemuxPlugin: 1, token: "other", kind: "ready" });
    host.receive({ kind: "ready" });
    expect(posted).toEqual([]);
  });

  it("answers ready with the context", () => {
    const { posted, send } = setup();
    send({ kind: "ready" });
    expect(posted).toEqual([{ codemuxPlugin: 1, kind: "context", context }]);
  });

  it("calls only well-formed handler names and returns the result's visible parts", async () => {
    const { posted, send, invoke } = setup();
    send({ kind: "invoke", id: 1, name: "../escape" });
    send({ kind: "invoke", id: 2, name: "load", input: { q: 1 } });
    await vi.waitFor(() => expect(posted).toHaveLength(2));
    expect(posted[0]).toMatchObject({ kind: "result", id: 1, ok: false, error: { reason: "invalid_handler" } });
    expect(invoke).toHaveBeenCalledWith("load", { q: 1 }, "origin");
    expect(posted[1]).toEqual({ codemuxPlugin: 1, kind: "result", id: 2, ok: true, value: { message: "ok", data: { n: 1 } } });
  });

  it("hands result intents to the client, never back to the frame", async () => {
    const { posted, send, onIntents } = setup({
      invoke: async () => ({ intents: [{ type: "view.open", viewId: "x" }], unsupportedIntentCount: 2 }),
    });
    send({ kind: "invoke", id: 1, name: "go" });
    await vi.waitFor(() => expect(posted).toHaveLength(1));
    expect(onIntents).toHaveBeenCalledWith([{ type: "view.open", viewId: "x" }], "origin", 2);
    expect(posted[0]).toMatchObject({ ok: true, value: {} });
  });

  it("validates direct intents and refuses them without the feature", () => {
    const { posted, send, onIntents } = setup();
    send({ kind: "intent", id: 1, intent: { type: "thread.reveal", threadId: "t-1" } });
    send({ kind: "intent", id: 2, intent: { type: "shell.exec" } });
    expect(onIntents).toHaveBeenCalledWith([{ type: "thread.reveal", threadId: "t-1" }], "origin", 0);
    expect(posted[1]).toMatchObject({ ok: false, error: { reason: "unsupported" } });

    const bare = setup({ features: [] });
    bare.send({ kind: "intent", id: 3, intent: { type: "view.open", viewId: "x" } });
    expect(bare.onIntents).not.toHaveBeenCalled();
    expect(bare.posted[0]).toMatchObject({ ok: false, error: { reason: "unsupported" } });
  });

  it("caps requests in flight", () => {
    const { posted, send } = setup({ invoke: () => new Promise(() => {}) });
    for (let id = 1; id <= 9; id += 1) send({ kind: "invoke", id, name: "slow" });
    expect(posted).toEqual([{ codemuxPlugin: 1, kind: "result", id: 9, ok: false, error: { reason: "busy", message: "Too many requests in flight." } }]);
  });

  it("clamps card resizes and only with the feature", () => {
    const onResize = vi.fn();
    setup({ onResize }).send({ kind: "resize", height: 50 });
    expect(onResize).not.toHaveBeenCalled();
    setup({ onResize, features: ["resize"] }).send({ kind: "resize", height: 99_999 });
    expect(onResize).toHaveBeenCalledWith(MAX_PLUGIN_CARD_HEIGHT);
  });

  it("goes silent once disposed", () => {
    const { host, posted, send } = setup();
    host.dispose();
    send({ kind: "ready" });
    expect(posted).toEqual([]);
  });
});

describe("buildPluginViewDocument", () => {
  it("puts the policy and bridge before any author markup", () => {
    const document = buildPluginViewDocument("<!DOCTYPE html><head><meta http-equiv='x'></head><body>hi</body>", "</script><b>", [
      "local-intents",
    ]);
    expect(document.startsWith("<!doctype html><html><head><meta charset=\"utf-8\">")).toBe(true);
    expect(document.indexOf(PLUGIN_VIEW_CONTENT_SECURITY_POLICY)).toBeLessThan(document.indexOf("<body>hi"));
    // The token cannot close the bridge script early.
    expect(document).not.toContain("</script><b>");
    expect(PLUGIN_VIEW_CONTENT_SECURITY_POLICY).toContain("connect-src 'none'");
  });

  it("gives cards resize and card context, every view local intents", () => {
    expect(pluginViewBridgeFeatures({ placement: "conversation", changes: true })).toEqual([
      "changes",
      "local-intents",
      "resize",
      "card-context",
    ]);
    expect(pluginViewBridgeFeatures({ placement: "page", changes: false })).toEqual(["local-intents"]);
  });
});

describe("acceptPluginViewPort", () => {
  it("accepts exactly one port with this document's token", () => {
    const { port1, port2 } = new MessageChannel();
    expect(acceptPluginViewPort({ codemuxPlugin: 1, kind: "connect", token: "tok" }, [port1], "tok")).toBe(port1);
    expect(acceptPluginViewPort({ codemuxPlugin: 1, kind: "connect", token: "nope" }, [port1], "tok")).toBeNull();
    expect(acceptPluginViewPort({ codemuxPlugin: 1, kind: "connect", token: "tok" }, [port1, port2], "tok")).toBeNull();
    port1.close();
    port2.close();
  });
});
