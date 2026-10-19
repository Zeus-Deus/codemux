import { describe, expect, it, vi } from "vitest";
import type { PluginMcpActivity, PluginMcpCallResult, PluginMcpServer, PluginOsInfo } from "@codemux/sdk";
import { ComputerUseController, type ComputerUseServices } from "./controller";

function deferred() {
  let resolve = () => {};
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

function fakeDesktop() {
  const registrations: {
    options: PluginMcpServer;
    activity?: (event: PluginMcpActivity) => void;
    calls: string[];
    stopped: boolean;
  }[] = [];
  const removed: string[] = [];
  let stopGate: Promise<void> | undefined;
  let registerGate: Promise<void> | undefined;
  let snapshotGate: Promise<void> | undefined;
  const rowFor = (id: string) => {
    const row = registrations.find((item) => item.options.id === id);
    if (!row) throw new Error(`Unknown test registration ${id}`);
    return row;
  };
  const services: ComputerUseServices = {
    process: {
      which: async () => "/demo/cua-driver",
      run: async ({ args }) => ({
        code: 0, stdout: args?.[0] === "--version" ? "cua-driver 0.21.0\n" : '{"ok":true,"probes":[]}',
        stderr: "", timedOut: false, truncated: false, pipesHeldOpen: false, cancelled: false,
      }),
    },
    os: { info: async () => ({ platform: "linux", arch: "x86_64", homeDir: "/home/demo", configDir: null, dataDir: null, localDataDir: null }) },
    fs: {
      writeDataFile: async (name) => `/plugin-data/${name}`,
      removeDataFile: async (name) => { removed.push(name); },
    },
    registerMcpServer: async (options) => {
      const row = { options, calls: [], stopped: false };
      registrations.push(row);
      if (options.scope !== "plugin") await registerGate;
      return async () => {
        if (options.scope !== "plugin") await stopGate;
        row.stopped = true;
      };
    },
    mcp: {
      callTool: async (id, tool): Promise<PluginMcpCallResult> => {
        rowFor(id).calls.push(tool);
        if (tool === "list_windows") return { content: [], structuredContent: { windows: [
          { pid: 42, window_id: 123, title: "Demo", app_name: "Demo App" },
        ] } };
        if (tool === "get_screen_size") return { content: [], structuredContent: { width: 1024, height: 768 } };
        await snapshotGate;
        return { content: [{ type: "image", mimeType: "image/png", data: "aGVsbG8=" }] };
      },
      getStatus: async (id) => ({
        id, name: rowFor(id).options.name, status: { kind: "running", toolCount: 3 },
        toolsCount: 3, errorMessage: null, stderrTail: null, startedAtMs: 0,
      }),
      onStatus: () => () => {},
      onActivity: (id, fn) => { const row = rowFor(id); row.activity = fn; return () => { row.activity = undefined; }; },
    },
  };
  return {
    services, registrations, removed,
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
  it.each<{
    platform: PluginOsInfo["platform"];
    homeDir: string | null;
    localDataDir: string | null;
    expected: string;
    requested?: string;
  }>([
    { platform: "linux", homeDir: "/home/Demo User/", localDataDir: null, expected: "/home/Demo User/.local/bin/cua-driver" },
    { platform: "linux", homeDir: "/home/demo", localDataDir: null, expected: "/home/demo/.local/bin/cua-driver", requested: "cua-driver" },
    { platform: "windows", homeDir: null, localDataDir: "C:\\Users\\Demo User\\AppData\\Local\\", expected: "C:\\Users\\Demo User\\AppData\\Local\\Programs\\Cua\\cua-driver\\bin\\cua-driver.exe" },
    { platform: "macos", homeDir: null, localDataDir: null, expected: "/Applications/CuaDriver.app/Contents/MacOS/cua-driver" },
    { platform: "macos", homeDir: "/Users/demo", localDataDir: null, expected: "/Users/demo/.local/bin/cua-driver" },
  ])("finds a standard $platform installation outside PATH", async ({ platform, homeDir, localDataDir, expected, requested }) => {
    const fake = fakeDesktop();
    fake.services.process.which = vi.fn(async (command) => command === expected ? expected : null);
    fake.services.os.info = async () => ({ platform, arch: "x86_64", homeDir, configDir: null, dataDir: null, localDataDir });
    const run = vi.spyOn(fake.services.process, "run");
    const controller = new ComputerUseController(fake.services);
    await controller.detect(requested);
    expect(fake.services.process.which).toHaveBeenNthCalledWith(1, "cua-driver");
    expect(controller.getState().error).toBeNull();
    expect(controller.getState().executable).toBe(expected);
    expect(run).toHaveBeenCalledWith(expect.objectContaining({ command: expected, args: ["--version"] }));
    expect(run).toHaveBeenCalledWith(expect.objectContaining({ command: expected, args: ["doctor", "--json"] }));
    await controller.dispose();
  });

  it.each([undefined, "/chosen/cua-driver"])("keeps PATH or custom executable precedence (%s)", async (command) => {
    const fake = fakeDesktop();
    fake.services.process.which = vi.fn(async () => "/resolved/cua-driver");
    const directories = vi.spyOn(fake.services.os, "info");
    const controller = new ComputerUseController(fake.services);
    await controller.detect(command);
    expect(fake.services.process.which).toHaveBeenCalledExactlyOnceWith(command ?? "cua-driver");
    expect(directories).not.toHaveBeenCalled();
    expect(controller.getState().executable).toBe("/resolved/cua-driver");
    await controller.dispose();
  });

  it("does not silently replace a missing custom executable", async () => {
    const fake = fakeDesktop();
    fake.services.process.which = vi.fn(async () => null);
    const directories = vi.spyOn(fake.services.os, "info");
    const run = vi.spyOn(fake.services.process, "run");
    const controller = new ComputerUseController(fake.services);
    await controller.detect("/missing/cua-driver");
    expect(directories).not.toHaveBeenCalled();
    expect(run).not.toHaveBeenCalled();
    expect(controller.getState().error).toContain("No executable was found at /missing/cua-driver");
    await controller.dispose();
  });

  it("handles unavailable home directories without constructing a relative fallback", async () => {
    const fake = fakeDesktop();
    fake.services.process.which = vi.fn(async () => null);
    fake.services.os.info = async () => ({ platform: "linux", arch: "aarch64", homeDir: null, configDir: null, dataDir: null, localDataDir: null });
    const run = vi.spyOn(fake.services.process, "run");
    const controller = new ComputerUseController(fake.services);
    await controller.detect();
    expect(fake.services.process.which).toHaveBeenCalledExactlyOnceWith("cua-driver");
    expect(run).not.toHaveBeenCalled();
    expect(controller.getState().error).toContain("Install it or enter its executable path");
    await controller.dispose();
  });

  it("never registers tools without workspace opt-in", async () => {
    const fake = fakeDesktop();
    const controller = new ComputerUseController(fake.services);
    await controller.setWorkspace({ id: "workspace-a", title: "Demo", local: true }, false);
    await controller.detect();
    await controller.discover();
    expect(fake.registrations).toHaveLength(0);
    expect(controller.getState().error).toContain("Allow computer use in this workspace");
    await controller.dispose();
  });

  it("does not route a remote workspace's agents to this desktop", async () => {
    const fake = fakeDesktop();
    const controller = new ComputerUseController(fake.services);
    await controller.detect();
    await controller.setWorkspace({ id: "remote", title: "Remote workspace", local: false }, true);
    await controller.discover();
    expect(fake.registrations).toHaveLength(0);
    expect(controller.getState().error).toContain("only in local workspaces");
    await controller.dispose();
  });

  it("does not assume another driver version honours the evaluated policy", async () => {
    const fake = fakeDesktop();
    const run = fake.services.process.run;
    fake.services.process.run = async (options) => options.args?.[0] === "--version"
      ? { code: 0, stdout: "cua-driver 0.22.0", stderr: "", timedOut: false, truncated: false, pipesHeldOpen: false, cancelled: false }
      : run(options);
    const controller = new ComputerUseController(fake.services);
    await controller.setWorkspace({ id: "workspace-a", title: "Demo", local: true }, true);
    await controller.detect();
    expect(controller.getState().error).toContain("0.22.0 has not been validated");
    await controller.discover();
    expect(fake.registrations).toHaveLength(0);
    await controller.dispose();
  });

  it.each(["--version", "doctor"])("refuses incomplete %s output when a descendant retains the pipes", async (stage) => {
    const fake = fakeDesktop();
    const run = fake.services.process.run;
    fake.services.process.run = async (options) => ({
      ...await run(options), pipesHeldOpen: options.args?.[0] === stage,
    });
    const controller = new ComputerUseController(fake.services);
    await controller.setWorkspace({ id: "workspace-a", title: "Demo", local: true }, true);
    await controller.detect();
    expect(controller.getState().doctor).toBeNull();
    expect(controller.getState().error).not.toBeNull();
    await controller.discover();
    expect(fake.registrations).toHaveLength(0);
    await controller.dispose();
  });

  it("keeps discovery and preview out of agent scope and tracks overlapping calls", async () => {
    const fake = fakeDesktop();
    const controller = new ComputerUseController(fake.services);
    const target = await chooseWindow(controller);
    expect(fake.registrations[0].options.scope).toBe("plugin");
    expect(fake.registrations[0].stopped).toBe(true);
    await controller.enable(target);
    const control = fake.registrations.find((row) => row.options.scope !== "plugin");
    const view = fake.registrations.find((row) => row.options.id.startsWith("cua-view-"));
    expect(control?.options.scope).toEqual({ workspaceIds: ["workspace-a"] });
    expect(control?.options.transport === "stdio" ? control.options.args : undefined).toEqual(["mcp", "--direct", "--no-overlay"]);
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
    const controller = new ComputerUseController(fake.services);
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
    const controller = new ComputerUseController(fake.services);
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
    const controller = new ComputerUseController(fake.services);
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
    const controller = new ComputerUseController(fake.services);
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
