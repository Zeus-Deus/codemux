import { afterEach, describe, expect, it, vi } from "vitest";
import type { PluginInvokeResult } from "./contract";
import { pluginThreadStatusEntries } from "./model";
import { createPluginThreadStatusPoller, PLUGIN_THREAD_STATUS_INTERVAL_MS } from "./thread-status";
import { installation, manifest } from "./test-fixtures";

const plugin = installation({ manifest: manifest({ threadStatus: [{ id: "live", title: "Live", viewId: "panel" }] }) });
const entries = pluginThreadStatusEntries([plugin]);

afterEach(() => {
  vi.useRealTimers();
});

describe("createPluginThreadStatusPoller", () => {
  it("asks the open chat's statuses only while visible, then on an interval", async () => {
    vi.useFakeTimers();
    const invoke = vi.fn(async (): Promise<PluginInvokeResult> => ({ data: { label: "Live", tone: "success" } }));
    const poller = createPluginThreadStatusPoller({ invoke, operationId: () => "op-1" });
    poller.update({ target: { threadId: "t-1", projectId: "p" }, entries, visible: false });
    expect(invoke).not.toHaveBeenCalled();
    poller.update({ target: { threadId: "t-1", projectId: "p" }, entries, visible: true });
    expect(invoke).toHaveBeenCalledWith({
      installationId: "inst-1",
      generation: "gen-1",
      handler: "status:live",
      context: { threadId: "t-1", projectId: "p" },
      operationId: "op-1",
    });
    await vi.waitFor(() => expect(poller.getSnapshot()).toHaveLength(1));
    expect(poller.getSnapshot()[0]).toMatchObject({ threadId: "t-1", status: { label: "Live", tone: "success" } });
    await vi.advanceTimersByTimeAsync(PLUGIN_THREAD_STATUS_INTERVAL_MS);
    expect(invoke).toHaveBeenCalledTimes(2);
    poller.dispose();
  });

  it("shows nothing for failures or malformed answers", async () => {
    const answers: Array<Promise<PluginInvokeResult>> = [
      Promise.reject(new Error("crashed")),
      Promise.resolve({ data: { label: "x".repeat(41), tone: "success" } }),
    ];
    const invoke = vi.fn(() => answers.shift()!);
    const poller = createPluginThreadStatusPoller({ invoke, operationId: () => "op" });
    poller.update({ target: { threadId: "t-1", projectId: null }, entries, visible: true });
    await Promise.resolve();
    poller.refresh();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(poller.getSnapshot()).toEqual([]);
    poller.dispose();
  });

  it("drops a chat's answers as soon as the user switches chats", async () => {
    let settle: (value: PluginInvokeResult) => void = () => {};
    const invoke = vi.fn(() => new Promise<PluginInvokeResult>((resolve) => (settle = resolve)));
    const poller = createPluginThreadStatusPoller({ invoke, operationId: () => "op" });
    poller.update({ target: { threadId: "t-1", projectId: null }, entries, visible: true });
    poller.update({ target: { threadId: "t-2", projectId: null }, entries: [], visible: true });
    settle({ data: { label: "Late", tone: "info" } });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(poller.getSnapshot()).toEqual([]);
    poller.dispose();
  });
});
