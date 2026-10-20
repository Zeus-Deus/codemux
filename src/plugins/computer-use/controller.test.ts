import { describe, expect, it, vi } from "vitest";
import type { PluginDirectoryEntry, PluginMcpActivity, PluginMcpCallResult, PluginMcpServer, PluginMcpStatus, PluginOsInfo } from "@codemux/sdk";
import { ComputerUseController, type ComputerUseServices } from "./controller";
import { CONTROL_LIFETIME_MS, CONTROL_TOOLS, DISCOVERY_TOOLS } from "./model";

function deferred() {
  let resolve = () => {};
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

function fakeDesktop() {
  const registrations: {
    options: PluginMcpServer;
    activity?: (event: PluginMcpActivity) => void;
    status?: (status: PluginMcpStatus | null) => void;
    stopCalls: number;
    calls: string[];
    stopped: boolean;
  }[] = [];
  const removed: string[] = [];
  const written = new Map<string, string>();
  const files = new Map<string, PluginDirectoryEntry["kind"]>();
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
      dataDir: async () => "/plugin-data/computer-use",
      listDir: async () => [...files].map(([name, kind]) => ({ name, kind })),
      writeDataFile: async (name, contents) => { files.set(name, "file"); written.set(name, contents); return `/plugin-data/computer-use/${name}`; },
      removeDataFile: async (name) => { removed.push(name); files.delete(name); },
    },
    registerMcpServer: async (options) => {
      const row = { options, calls: [], stopped: false, stopCalls: 0 };
      registrations.push(row);
      if (options.scope !== "plugin") await registerGate;
      return async () => {
        row.stopCalls++;
        if (options.scope !== "plugin") await stopGate;
        row.stopped = true;
      };
    },
    mcp: {
      listTools: async (id) => (id.startsWith("cua-control-") ? CONTROL_TOOLS : id.startsWith("cua-view-") ? ["get_window_state"] : DISCOVERY_TOOLS).map((name) => ({ name, prefixedName: `computer-use__${name}`, description: null, inputSchema: {}, serverId: id })),
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
      onStatus: (id, fn) => { const row = rowFor(id); row.status = fn; return () => { row.status = undefined; }; },
      onActivity: (id, fn) => { const row = rowFor(id); row.activity = fn; return () => { row.activity = undefined; }; },
    },
  };
  return {
    services, registrations, removed, files, written,
    holdStop: (gate?: Promise<void>) => { stopGate = gate; },
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
  it("sweeps and logs only its exact regular policy/manifest filenames in its own data directory", async () => {
    const fake = fakeDesktop();
    const uuid = "00000000-0000-4000-8000-000000000001";
    const stale = ["discovery", "view", "control"].flatMap((kind) => ["yaml", "json"].map((ext) => `cua-${kind}-${uuid}.${ext}`));
    for (const name of stale) fake.files.set(name, "file");
    const untouched: [string, PluginDirectoryEntry["kind"]][] = [
      ["notes.json", "file"], [`other-plugin-${uuid}.yaml`, "file"],
      ["cua-control-invalid.json", "file"], [`cua-control-${uuid}.json.bak`, "file"],
      [`../cua-control-${uuid}.json`, "file"],
      ["cua-control-00000000-0000-4000-8000-000000000002.json", "symlink"],
      ["cua-view-00000000-0000-4000-8000-000000000003.yaml", "directory"],
      ["cua-discovery-00000000-0000-4000-8000-000000000004.json", "other"],
    ];
    for (const [name, kind] of untouched) fake.files.set(name, kind);
    const list = vi.spyOn(fake.services.fs, "listDir");
    const log = vi.spyOn(console, "info").mockImplementation(() => {});
    try {
      const controller = new ComputerUseController(fake.services);
      await controller.sweepStaleFiles();
      expect(list).toHaveBeenCalledExactlyOnceWith("/plugin-data/computer-use");
      expect(fake.removed).toEqual(stale);
      expect([...fake.files]).toEqual(untouched);
      expect(log.mock.calls).toEqual(stale.map((name) => ["[computer-use] Removed stale driver file", name]));
      expect(fake.registrations).toHaveLength(0);
      await controller.dispose();
    } finally { log.mockRestore(); }
  });

  it("preserves another incarnation's pending, live and stopping registrations until confirmed exit", async () => {
    const fake = fakeDesktop();
    const first = new ComputerUseController(fake.services);
    const next = new ComputerUseController(fake.services);
    const target = await chooseWindow(first);
    fake.removed.length = 0;
    const registering = deferred();
    fake.holdRegistration(registering.promise);
    const enabling = first.enable(target);
    await vi.waitFor(() => expect(fake.registrations.some((row) => row.options.scope !== "plugin")).toBe(true));
    const names = [...fake.files.keys()];
    expect(names).toHaveLength(4);
    await next.sweepStaleFiles();
    expect(fake.removed).toEqual([]);
    registering.resolve();
    await enabling;
    await next.sweepStaleFiles();
    expect(fake.removed).toEqual([]);
    const stopping = deferred();
    fake.holdStop(stopping.promise);
    const stopped = first.stop();
    await vi.waitFor(() => expect(fake.registrations.find((row) => row.options.id.startsWith("cua-view-"))?.stopped).toBe(true));
    const controlNames = names.filter((name) => name.startsWith("cua-control-"));
    await next.sweepStaleFiles();
    expect(controlNames.every((name) => fake.files.has(name))).toBe(true);
    expect(fake.removed.some((name) => controlNames.includes(name))).toBe(false);
    stopping.resolve();
    await stopped;
    expect(fake.files.size).toBe(0);
    await Promise.all([first.dispose(), next.dispose()]);
  });

  it("cleans files left when an ended owner could stop children but could not remove data files", async () => {
    const fake = fakeDesktop();
    const first = new ComputerUseController(fake.services);
    await first.enable(await chooseWindow(first));
    const names = [...fake.files.keys()];
    const remove = fake.services.fs.removeDataFile;
    fake.services.fs.removeDataFile = async () => { throw new Error("Plugin host owner is no longer active"); };
    await first.dispose();
    expect(fake.registrations.every((row) => row.stopped)).toBe(true);
    expect([...fake.files.keys()]).toEqual(names);
    fake.services.fs.removeDataFile = remove;
    fake.removed.length = 0;
    const log = vi.spyOn(console, "info").mockImplementation(() => {});
    try {
      const next = new ComputerUseController(fake.services);
      await next.sweepStaleFiles();
      expect(fake.removed).toEqual(names);
      expect(fake.files.size).toBe(0);
      expect(log).toHaveBeenCalledTimes(4);
      await next.dispose();
    } finally { log.mockRestore(); }
  });

  it.each(["probe", "control"])("fails closed when the %s child exposes an extra tool", async (stage) => {
    const fake = fakeDesktop();
    const controller = new ComputerUseController(fake.services);
    const target = await chooseWindow(controller);
    const list = fake.services.mcp.listTools;
    fake.services.mcp.listTools = async (id) => {
      const tools = await list(id);
      const bad = id.startsWith("cua-control-") && (stage === "probe" ? id.endsWith("-probe") : !id.endsWith("-probe"));
      return bad ? [...tools, { name: "press_key", prefixedName: "computer-use__press_key", description: null, inputSchema: {}, serverId: id }] : tools;
    };
    await controller.enable(target);
    expect(controller.getState().phase).toBe("error");
    expect(controller.getState().error).toContain("unexpected tools: press_key");
    expect(fake.registrations.every((row) => row.stopped)).toBe(true);
    expect(fake.registrations.filter((row) => row.options.scope !== "plugin")).toHaveLength(stage === "probe" ? 0 : 1);
    expect(fake.files.size).toBe(0);
    await controller.dispose();
  });

  it("probes the exact private files before exposing the control child and pins its environment", async () => {
    const fake = fakeDesktop();
    const register = fake.services.registerMcpServer;
    fake.services.registerMcpServer = async (options) => {
      if (options.scope !== "plugin") expect(fake.registrations.filter((row) => row.options.id.endsWith("-probe")).every((row) => row.stopped)).toBe(true);
      return register(options);
    };
    const controller = new ComputerUseController(fake.services);
    await controller.enable(await chooseWindow(controller));
    const control = fake.registrations.find((row) => row.options.scope !== "plugin")?.options;
    const probe = fake.registrations.find((row) => row.options.id.endsWith("-probe"))?.options;
    if (!control || control.transport !== "stdio" || !probe || probe.transport !== "stdio") throw new Error("Missing direct registrations");
    expect(probe.scope).toBe("plugin");
    expect(control.env).toEqual({
      CUA_DRIVER_PERMISSION_MODE: "bounded", CUA_DRIVER_CAPABILITY_MANIFEST_APPROVED: "1",
      CUA_DRIVER_POLICY_FILE: `/plugin-data/computer-use/${control.id}.yaml`,
      CUA_DRIVER_CAPABILITY_MANIFEST_FILE: `/plugin-data/computer-use/${control.id}.json`,
      CUA_DRIVER_RS_TELEMETRY_ENABLED: "0", CUA_DRIVER_EMBEDDED: "1",
    });
    expect(probe.env).toEqual(control.env);
    expect(fake.written.has(`${control.id}.yaml`)).toBe(true);
    expect(fake.written.has(`${control.id}.json`)).toBe(true);
    expect(JSON.parse(fake.written.get(`${control.id}.json`) ?? "{}").allow.tools).toEqual(["get_window_state", "click", "type_text", "scroll"]);
    await controller.dispose();
  });

  it.each(["yaml", "json"])("never exposes control if its %s file cannot be written", async (extension) => {
    const fake = fakeDesktop();
    const controller = new ComputerUseController(fake.services);
    const target = await chooseWindow(controller);
    const write = fake.services.fs.writeDataFile;
    fake.services.fs.writeDataFile = async (name, contents) => {
      if (name.startsWith("cua-control-") && name.endsWith(`.${extension}`)) throw new Error("Write failed");
      return write(name, contents);
    };
    await controller.enable(target);
    expect(controller.getState().error).toBe("Write failed");
    expect(fake.registrations.some((row) => row.options.scope !== "plugin")).toBe(false);
    expect(fake.registrations.every((row) => row.stopped)).toBe(true);
    expect(fake.files.size).toBe(0);
    await controller.dispose();
  });

  it.each(["workspace", "opt-in"])("stops active input when %s changes", async (change) => {
    const fake = fakeDesktop();
    const controller = new ComputerUseController(fake.services);
    await controller.enable(await chooseWindow(controller));
    await controller.setWorkspace({ id: change === "workspace" ? "workspace-b" : "workspace-a", title: "Demo", local: true }, change !== "opt-in");
    expect(fake.registrations.every((row) => row.stopped)).toBe(true);
    expect(controller.getState().target).toBeNull();
    expect(controller.getState().windows).toEqual([]);
    await controller.dispose();
  });

  it.each(["stopped", "errored"] as const)("handles the control status %s", async (kind) => {
    const fake = fakeDesktop();
    const controller = new ComputerUseController(fake.services);
    await controller.enable(await chooseWindow(controller));
    const control = fake.registrations.find((row) => row.options.scope !== "plugin");
    if (!control) throw new Error("Missing control");
    const status = kind === "errored" ? { kind, message: "Driver failed" } : { kind };
    control.status?.({ id: control.options.id, name: "Computer use", status, toolsCount: 0, errorMessage: null, stderrTail: null, startedAtMs: 0 });
    if (kind === "stopped") {
      await vi.waitFor(() => expect(controller.getState().phase).toBe("idle"));
      expect(fake.registrations.every((row) => row.stopped)).toBe(true);
    } else {
      expect(controller.getState().phase).toBe("error");
      expect(controller.getState().error).toBe("Driver failed");
      expect(controller.getState().target).not.toBeNull();
    }
    await controller.dispose();
  });

  it("releases the old input before re-choosing and registering new input", async () => {
    const fake = fakeDesktop();
    const controller = new ComputerUseController(fake.services);
    await controller.enable(await chooseWindow(controller));
    const old = fake.registrations.find((row) => row.options.scope !== "plugin");
    const register = fake.services.registerMcpServer;
    fake.services.registerMcpServer = async (options) => {
      expect(old?.stopped).toBe(true);
      return register(options);
    };
    await controller.discover();
    await controller.enable(controller.getState().windows[0]);
    expect(controller.getState().phase).toBe("active");
    expect(fake.registrations.filter((row) => row.options.scope !== "plugin")).toHaveLength(2);
    await controller.dispose();
  });

  it("does not flash Stopping when an idle workspace is first selected or switched", async () => {
    const fake = fakeDesktop();
    const controller = new ComputerUseController(fake.services);
    const phases: string[] = [];
    controller.subscribe(() => phases.push(controller.getState().phase));
    await controller.setWorkspace({ id: "a", title: "A", local: true }, false);
    await controller.setWorkspace({ id: "b", title: "B", local: true }, false);
    expect(phases).not.toContain("stopping");
    await controller.dispose();
  });

  it("stops at the ten-minute UI deadline and requires another window selection", async () => {
    vi.useFakeTimers();
    try {
      const fake = fakeDesktop();
      const controller = new ComputerUseController(fake.services);
      const target = await chooseWindow(controller);
      const now = Date.now();
      await controller.enable(target);
      expect(controller.getState().expiresAt).toBe(now + CONTROL_LIFETIME_MS);
      await vi.advanceTimersByTimeAsync(CONTROL_LIFETIME_MS);
      expect(controller.getState().phase).toBe("expired");
      expect(controller.getState().target).toBeNull();
      expect(controller.getState().windows).toEqual([]);
      expect(fake.registrations.every((row) => row.stopped)).toBe(true);
      await controller.enable(target);
      expect(controller.getState().error).toContain("choose a current window");
      await controller.dispose();
    } finally { vi.useRealTimers(); }
  });

  it("disables telemetry for both diagnostics and rejects a nonzero doctor exit", async () => {
    const fake = fakeDesktop();
    const run = fake.services.process.run;
    fake.services.process.run = vi.fn(async (options) => ({ ...await run(options), code: options.args?.[0] === "doctor" ? 1 : 0 }));
    const controller = new ComputerUseController(fake.services);
    await controller.detect();
    expect(fake.services.process.run).toHaveBeenCalledTimes(2);
    for (const [options] of vi.mocked(fake.services.process.run).mock.calls) expect(options.env?.CUA_DRIVER_RS_TELEMETRY_ENABLED).toBe("0");
    expect(controller.getState().doctor?.ok).toBe(false);
    expect(controller.getState().error).toContain("doctor exited with code 1");
    expect(controller.getState().phase).toBe("error");
    await controller.dispose();
  });

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
    expect(control?.options.toolDescriptions?.click.append).toContain("structured tools (browser tools, app-specific plugin tools)");
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
    const control = fake.registrations.find((row) => row.options.scope !== "plugin");
    const attempts = control?.stopCalls ?? 0;
    fake.holdStop();
    await controller.stop();
    expect(control?.stopCalls).toBe(attempts + 1);
    expect(control?.stopped).toBe(true);
    expect(controller.getState().phase).toBe("idle");
    expect(controller.getState().windows).toEqual([]);
    expect(controller.getState().target).toBeNull();
    await controller.dispose();
  });
});
