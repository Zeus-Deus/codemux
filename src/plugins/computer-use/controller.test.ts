import { describe, expect, it, vi } from "vitest";
import type { PluginMcpActivity, PluginMcpCallResult } from "@codemux/sdk";
import type { DesktopAdapter, DriverConnection, DriverOptions } from "./adapter";
import { ComputerUseController } from "./controller";

function deferred() {
  let resolve = () => {};
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

function fakeDesktop() {
  const registrations: {
    options: DriverOptions;
    connection: DriverConnection;
    activity?: (event: PluginMcpActivity) => void;
    calls: string[];
    stopped: boolean;
  }[] = [];
  const removed: string[] = [];
  let stopGate: Promise<void> | undefined;
  let registerGate: Promise<void> | undefined;
  let snapshotGate: Promise<void> | undefined;
  const desktop: DesktopAdapter = {
    which: async () => "/demo/cua-driver",
    run: async ({ args }) => ({
      code: 0, stdout: args?.[0] === "--version" ? "cua-driver 0.21.0\n" : '{"ok":true,"probes":[]}',
      stderr: "", timedOut: false, truncated: false, cancelled: false,
    }),
    writeDataFile: async (name) => `/plugin-data/${name}`,
    removeDataFile: async (name) => { removed.push(name); },
    register: async (options) => {
      const row: (typeof registrations)[number] = {
        options, calls: [], stopped: false,
        connection: {
          call: async (tool): Promise<PluginMcpCallResult> => {
            row.calls.push(tool);
            if (tool === "list_windows") return { content: [], structuredContent: { windows: [
              { pid: 42, window_id: 123, title: "Demo", app_name: "Demo App" },
            ] } };
            if (tool === "get_screen_size") return { content: [], structuredContent: { width: 1024, height: 768 } };
            await snapshotGate;
            return { content: [{ type: "image", mimeType: "image/png", data: "aGVsbG8=" }] };
          },
          status: async () => ({
            id: options.id, name: options.name, status: { kind: "running", toolCount: 3 },
            toolsCount: 3, errorMessage: null, stderrTail: null, startedAtMs: 0,
          }),
          onStatus: () => () => {},
          onActivity: (fn) => { row.activity = fn; return () => { row.activity = undefined; }; },
          stop: async () => {
            if (options.scope !== "plugin") await stopGate;
            row.stopped = true;
          },
        },
      };
      registrations.push(row);
      if (options.scope !== "plugin") await registerGate;
      return row.connection;
    },
  };
  return {
    desktop, registrations, removed,
    holdStop: (gate: Promise<void>) => { stopGate = gate; },
    holdRegistration: (gate: Promise<void>) => { registerGate = gate; },
    holdSnapshot: (gate: Promise<void>) => { snapshotGate = gate; },
  };
}

async function chooseWindow(controller: ComputerUseController) {
  await controller.setWorkspace({ id: "workspace-a", title: "Demo workspace", local: true }, true);
  await controller.detect();
  await controller.discover();
  const target = controller.getState().windows[0];
  expect(target).toBeDefined();
  return target;
}

describe("computer-use lifecycle", () => {
  it("never registers tools without workspace opt-in", async () => {
    const fake = fakeDesktop();
    const controller = new ComputerUseController(fake.desktop);
    await controller.setWorkspace({ id: "workspace-a", title: "Demo", local: true }, false);
    await controller.detect();
    await controller.discover();
    expect(fake.registrations).toHaveLength(0);
    expect(controller.getState().error).toContain("Allow computer use in this workspace");
    await controller.dispose();
  });

  it("does not route a remote workspace's agents to this desktop", async () => {
    const fake = fakeDesktop();
    const controller = new ComputerUseController(fake.desktop);
    await controller.detect();
    await controller.setWorkspace({ id: "remote", title: "Remote workspace", local: false }, true);
    await controller.discover();
    expect(fake.registrations).toHaveLength(0);
    expect(controller.getState().error).toContain("only in local workspaces");
    await controller.dispose();
  });

  it("does not assume another driver version honours the evaluated policy", async () => {
    const fake = fakeDesktop();
    const run = fake.desktop.run;
    fake.desktop.run = async (options) => options.args?.[0] === "--version"
      ? { code: 0, stdout: "cua-driver 0.22.0", stderr: "", timedOut: false, truncated: false, cancelled: false }
      : run(options);
    const controller = new ComputerUseController(fake.desktop);
    await controller.setWorkspace({ id: "workspace-a", title: "Demo", local: true }, true);
    await controller.detect();
    expect(controller.getState().error).toContain("0.22.0 has not been validated");
    await controller.discover();
    expect(fake.registrations).toHaveLength(0);
    await controller.dispose();
  });

  it("keeps discovery and preview out of agent scope and tracks overlapping calls", async () => {
    const fake = fakeDesktop();
    const controller = new ComputerUseController(fake.desktop);
    const target = await chooseWindow(controller);
    expect(fake.registrations[0].options.scope).toBe("plugin");
    expect(fake.registrations[0].stopped).toBe(true);
    await controller.enable(target);
    const control = fake.registrations.find((row) => row.options.scope !== "plugin");
    const view = fake.registrations.find((row) => row.options.id.startsWith("cua-view-"));
    expect(control?.options.scope).toEqual({ workspaceIds: ["workspace-a"] });
    expect(control?.options.args).toEqual(["mcp", "--direct", "--no-overlay"]);
    expect(control?.options.toolDescriptions?.click.append).toContain("structured browser/Unity tools first");
    expect(control?.options.shutdown).toBe("immediate");
    await controller.refreshPreview();
    expect(view?.calls).toEqual(["get_window_state", "get_window_state"]);
    expect(control?.calls).toEqual([]);
    const event = { toolName: "click", prefixedName: "mcp__computer-use__click", workspaceId: "workspace-a" };
    control?.activity?.({ ...event, phase: "start", callId: "a" });
    control?.activity?.({ ...event, phase: "start", callId: "b" });
    control?.activity?.({ ...event, phase: "end", callId: "a" });
    expect(controller.getState().activeCalls).toBe(1);
    control?.activity?.({ ...event, phase: "end", callId: "b" });
    expect(controller.getState().activeCalls).toBe(0);
    await controller.dispose();
  });

  it("keeps Stop pending until the input child confirms exit", async () => {
    const fake = fakeDesktop();
    const controller = new ComputerUseController(fake.desktop);
    await controller.enable(await chooseWindow(controller));
    const gate = deferred();
    fake.holdStop(gate.promise);
    const stopping = controller.stop();
    await vi.waitFor(() => expect(controller.getState().phase).toBe("stopping"));
    expect(fake.registrations.find((row) => row.options.scope !== "plugin")?.stopped).toBe(false);
    gate.resolve();
    await stopping;
    expect(controller.getState().phase).toBe("idle");
    expect(fake.registrations.every((row) => row.stopped)).toBe(true);
    expect(controller.getState().target).toBeNull();
    expect(fake.removed).toHaveLength(6);
    await controller.dispose();
  });

  it("stops a registration that finishes after the user has pressed Stop", async () => {
    const fake = fakeDesktop();
    const controller = new ComputerUseController(fake.desktop);
    const target = await chooseWindow(controller);
    const gate = deferred();
    fake.holdRegistration(gate.promise);
    const enabling = controller.enable(target);
    await vi.waitFor(() => expect(fake.registrations.some((row) => row.options.scope !== "plugin")).toBe(true));
    const stopping = controller.stop();
    expect(controller.getState().phase).toBe("stopping");
    gate.resolve();
    await Promise.all([enabling, stopping]);
    expect(controller.getState().phase).toBe("idle");
    expect(fake.registrations.every((row) => row.stopped)).toBe(true);
    await controller.dispose();
  });

  it("dispatches Stop while a setup capture is still pending", async () => {
    const fake = fakeDesktop();
    const controller = new ComputerUseController(fake.desktop);
    const target = await chooseWindow(controller);
    const gate = deferred();
    fake.holdSnapshot(gate.promise);
    const enabling = controller.enable(target);
    await vi.waitFor(() => expect(fake.registrations.some((row) => row.calls.includes("get_window_state"))).toBe(true));
    const stopping = controller.stop();
    await vi.waitFor(() => expect(fake.registrations.every((row) => row.stopped)).toBe(true));
    expect(controller.getState().phase).toBe("stopping");
    gate.resolve();
    await Promise.all([enabling, stopping]);
    expect(controller.getState().phase).toBe("idle");
    expect(fake.registrations.filter((row) => row.options.scope !== "plugin")).toHaveLength(0);
    await controller.dispose();
  });

  it("keeps the target visible when child shutdown cannot be confirmed", async () => {
    const fake = fakeDesktop();
    const controller = new ComputerUseController(fake.desktop);
    await controller.enable(await chooseWindow(controller));
    let rejectStop = (_reason: Error) => {};
    fake.holdStop(new Promise<void>((_resolve, reject) => { rejectStop = reject; }));
    const stopping = controller.stop();
    rejectStop(new Error("Could not confirm child exit"));
    await stopping;
    expect(controller.getState().phase).toBe("error");
    expect(controller.getState().error).toBe("Could not confirm child exit");
    expect(controller.getState().target?.windowId).toBe(123);
    expect(fake.registrations.find((row) => row.options.scope !== "plugin")?.stopped).toBe(false);
    await expect(controller.dispose()).rejects.toThrow("Could not confirm child exit");
  });
});
