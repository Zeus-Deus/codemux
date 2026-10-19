import type { DesktopAdapter, DriverConnection } from "./adapter";
import {
  connectionFailure, controlFiles, controlInstructions, discoveryFiles, parseDoctor, parseVersion,
  readFrame, readWindows, toolText, viewFiles,
  OBSERVATION_TOOLS, INPUT_TOOLS,
  type DoctorReport, type PreviewFrame, type WindowTarget,
} from "./model";

export interface Workspace {
  id: string;
  title: string;
  local: boolean;
}

export interface ComputerUseState {
  workspace: Workspace | null;
  optedIn: boolean;
  executable: string | null;
  version: string | null;
  doctor: DoctorReport | null;
  phase: "idle" | "detecting" | "discovering" | "choosing" | "starting" | "active" | "stopping" | "error";
  windows: WindowTarget[];
  target: WindowTarget | null;
  activeCalls: number;
  frame: PreviewFrame | null;
  error: string | null;
}

interface Resource {
  connection: DriverConnection;
  files: string[];
  unsubscribe: (() => void)[];
  stopping?: Promise<void>;
}

class Cancelled extends Error {}

export class ComputerUseController {
  private state: ComputerUseState = {
    workspace: null, optedIn: false, executable: null, version: null, doctor: null,
    phase: "idle", windows: [], target: null, activeCalls: 0, frame: null, error: null,
  };
  private readonly listeners = new Set<() => void>();
  private readonly resources = new Set<Resource>();
  private readonly calls = new Set<string>();
  private abort = new AbortController();
  private operation: Promise<void> = Promise.resolve();
  private view: Resource | null = null;
  private previewBusy = false;
  private disposed = false;

  constructor(private readonly desktop: DesktopAdapter) {}

  getState = () => this.state;
  subscribe = (fn: () => void) => {
    this.listeners.add(fn);
    return () => { this.listeners.delete(fn); };
  };

  private update(patch: Partial<ComputerUseState>) {
    if (this.disposed) return;
    this.state = { ...this.state, ...patch };
    for (const fn of this.listeners) fn();
  }

  private check(signal: AbortSignal) {
    if (this.disposed || signal.aborted) throw new Cancelled();
  }

  private schedule(action: (signal: AbortSignal) => Promise<void>): Promise<void> {
    this.abort.abort();
    this.abort = new AbortController();
    const signal = this.abort.signal;
    const next = this.operation.then(async () => {
      this.check(signal);
      try { await action(signal); }
      catch (error) {
        if (error instanceof Cancelled || signal.aborted || this.disposed) return;
        await this.releaseAll().catch(() => {});
        this.update({ phase: "error", error: error instanceof Error ? error.message : String(error) });
      }
    });
    this.operation = next.catch(() => {});
    return this.operation;
  }

  setWorkspace(workspace: Workspace | null, optedIn: boolean): Promise<void> {
    if (this.state.workspace?.id === workspace?.id && this.state.workspace?.local === workspace?.local && this.state.optedIn === optedIn) return Promise.resolve();
    this.update({ workspace, optedIn, windows: [], frame: null });
    return this.stop();
  }

  detect(command?: string): Promise<void> {
    return this.schedule(async (signal) => {
      await this.releaseAll();
      this.check(signal);
      this.update({ phase: "detecting", error: null, executable: null, version: null, doctor: null });
      const executable = command?.trim() || await this.desktop.which("cua-driver");
      this.check(signal);
      if (!executable) throw new Error("cua-driver was not found. Install it, then check again.");
      const versionResult = await this.desktop.run({
        command: executable, args: ["--version"], timeoutMs: 5_000, maxOutputBytes: 16_384,
        workspaceId: this.state.workspace?.id, env: { CUA_DRIVER_EMBEDDED: "1" },
      });
      this.check(signal);
      const version = parseVersion(versionResult.stdout);
      if (versionResult.code !== 0 || !version || versionResult.cancelled || versionResult.truncated || versionResult.timedOut) {
        throw new Error(versionResult.stderr || "Could not read the cua-driver version.");
      }
      if (version !== "0.21.0") throw new Error(`cua-driver ${version} has not been validated with this plugin. Use version 0.21.0.`);
      const result = await this.desktop.run({
        command: executable, args: ["doctor", "--json"], timeoutMs: 15_000, maxOutputBytes: 262_144,
        env: { CUA_DRIVER_RS_TELEMETRY_ENABLED: "0", CUA_DRIVER_EMBEDDED: "1" }, workspaceId: this.state.workspace?.id,
      });
      this.check(signal);
      const doctor = parseDoctor(result.stdout);
      if (!doctor || result.cancelled || result.truncated || result.timedOut) {
        throw new Error(result.stderr || "cua-driver did not return a complete diagnostic report.");
      }
      this.update({ executable, version, doctor, phase: "idle", error: doctor.ok ? null : "cua-driver reported a setup problem. Review its diagnostics." });
    });
  }

  private authorized() {
    const { workspace, optedIn, executable } = this.state;
    if (!workspace || !optedIn) throw new Error("Allow computer use in this workspace first.");
    if (!workspace.local) throw new Error("Computer use is available only in local workspaces.");
    if (!executable) throw new Error("Check the cua-driver installation first.");
    if (this.state.doctor?.ok !== true) throw new Error("Resolve the driver setup problems before enabling computer use.");
    return { workspace, executable };
  }

  private async open(
    kind: "discovery" | "view" | "control",
    files: { policy: string; manifest: string },
    signal: AbortSignal,
    target?: WindowTarget,
  ): Promise<Resource> {
    const { workspace, executable } = this.authorized();
    const id = `cua-${kind}-${crypto.randomUUID()}`;
    const names = [`${id}.yaml`, `${id}.json`];
    let resource: Resource | null = null;
    try {
      const policy = await this.desktop.writeDataFile(names[0], files.policy);
      this.check(signal);
      const manifest = await this.desktop.writeDataFile(names[1], files.manifest);
      this.check(signal);
      const connection = await this.desktop.register({
        id, name: kind === "control" ? "Computer use" : "Computer use preview",
        scope: kind === "control" ? { workspaceIds: [workspace.id] } : "plugin",
        toolPrefix: kind === "control" ? "computer-use" : `computer-use-${kind}`,
        ...(kind === "control" && target ? {
          toolDescriptions: Object.fromEntries([...OBSERVATION_TOOLS, ...INPUT_TOOLS].map((tool) => [tool, { append: controlInstructions(target) }])),
        } : {}),
        shutdown: "immediate",
        transport: "stdio", command: executable, args: ["mcp", "--direct", "--no-overlay"],
        env: {
          CUA_DRIVER_PERMISSION_MODE: "bounded", CUA_DRIVER_CAPABILITY_MANIFEST_FILE: manifest,
          CUA_DRIVER_CAPABILITY_MANIFEST_APPROVED: "1", CUA_DRIVER_POLICY_FILE: policy,
          CUA_DRIVER_RS_TELEMETRY_ENABLED: "0", CUA_DRIVER_EMBEDDED: "1",
        },
      });
      resource = { connection, files: names, unsubscribe: [] };
      this.resources.add(resource);
      this.check(signal);
      const deadline = Date.now() + 15_000;
      while (Date.now() < deadline) {
        this.check(signal);
        const runtime = await connection.status();
        this.check(signal);
        if (runtime?.status.kind === "running") return resource;
        if (runtime?.status.kind === "errored") throw new Error(runtime.status.message);
        if (runtime?.status.kind === "stopped") throw new Error("cua-driver stopped during startup.");
        await this.delay(signal);
      }
      throw new Error("cua-driver did not become ready.");
    } catch (error) {
      if (resource) await this.release(resource);
      else await Promise.all(names.map((name) => this.desktop.removeDataFile(name).catch(() => {})));
      throw error;
    }
  }

  private delay(signal: AbortSignal): Promise<void> {
    return new Promise((resolve, reject) => {
      const cancel = () => { clearTimeout(timer); signal.removeEventListener("abort", cancel); reject(new Cancelled()); };
      const timer = setTimeout(() => { signal.removeEventListener("abort", cancel); resolve(); }, 150);
      signal.addEventListener("abort", cancel, { once: true });
      if (signal.aborted) cancel();
    });
  }

  discover(): Promise<void> {
    return this.schedule(async (signal) => {
      await this.releaseAll();
      this.check(signal);
      this.authorized();
      this.update({ phase: "discovering", windows: [], target: null, frame: null, error: null });
      const discovery = await this.open("discovery", discoveryFiles(), signal);
      const diagnostic = await discovery.connection.call("get_screen_size");
      this.check(signal);
      const failure = connectionFailure(diagnostic);
      if (failure) throw new Error(failure);
      const result = await discovery.connection.call("list_windows");
      this.check(signal);
      if (result.isError) throw new Error(toolText(result) ?? "The driver could not list application windows.");
      const windows = readWindows(result);
      await this.release(discovery);
      this.check(signal);
      this.update({ phase: "choosing", windows });
    });
  }

  enable(target: WindowTarget): Promise<void> {
    return this.schedule(async (signal) => {
      await this.releaseAll();
      this.check(signal);
      const { workspace } = this.authorized();
      if (!this.state.windows.some((window) => window.pid === target.pid && window.windowId === target.windowId)) {
        throw new Error("Refresh the window list and choose a current window.");
      }
      this.update({ phase: "starting", target, frame: null, error: null });
      // Polling in a separate read-only runtime leaves the agent's element tokens intact.
      const view = await this.open("view", viewFiles(target), signal, target);
      const args = { pid: target.pid, window_id: target.windowId, include_screenshot: true };
      const snapshot = await view.connection.call("get_window_state", args);
      this.check(signal);
      if (snapshot.isError) throw new Error(toolText(snapshot) ?? "The selected window is no longer available.");
      const control = await this.open("control", controlFiles(target), signal, target);
      control.unsubscribe.push(control.connection.onActivity((event) => {
        if (this.abort.signal !== signal || event.workspaceId !== workspace.id) return;
        if (event.phase === "start") this.calls.add(event.callId);
        else this.calls.delete(event.callId);
        this.update({ activeCalls: this.calls.size });
      }));
      control.unsubscribe.push(control.connection.onStatus((runtime) => {
        if (this.abort.signal !== signal || this.disposed) return;
        if (runtime?.status.kind === "errored") this.update({ phase: "error", error: runtime.status.message });
        if (runtime?.status.kind === "stopped") void this.stop();
      }));
      this.check(signal);
      this.view = view;
      this.update({ phase: "active", frame: readFrame(snapshot) });
    });
  }

  async refreshPreview(): Promise<void> {
    if (this.previewBusy || !this.view || this.state.phase !== "active" || !this.state.target) return;
    const view = this.view;
    const target = this.state.target;
    const signal = this.abort.signal;
    this.previewBusy = true;
    try {
      const result = await view.connection.call("get_window_state", {
        pid: target.pid, window_id: target.windowId, include_screenshot: true,
      });
      if (!signal.aborted && this.view === view) this.update({ frame: readFrame(result) });
    } catch (error) {
      if (!signal.aborted && this.view === view) this.update({ frame: { kind: "unavailable", message: error instanceof Error ? error.message : String(error) } });
    } finally { this.previewBusy = false; }
  }

  private release(resource: Resource): Promise<void> {
    resource.stopping ??= (async () => {
      await resource.connection.stop();
      for (const unsubscribe of resource.unsubscribe) unsubscribe();
      this.resources.delete(resource);
      if (this.view === resource) this.view = null;
      await Promise.all(resource.files.map((name) => this.desktop.removeDataFile(name).catch(() => {})));
    })();
    return resource.stopping;
  }

  private async releaseAll() {
    await Promise.all([...this.resources].map((resource) => this.release(resource)));
    this.calls.clear();
    this.update({ activeCalls: 0 });
  }

  stop(): Promise<void> {
    this.update({ phase: "stopping" });
    const immediate = this.releaseAll();
    // Observe a failure now, then surface it through the serialized operation.
    void immediate.catch(() => {});
    return this.schedule(async () => {
      await immediate;
      await this.releaseAll();
      this.update({ phase: "idle", activeCalls: 0, frame: null, target: null, error: null });
    });
  }

  async dispose(): Promise<void> {
    this.disposed = true;
    this.abort.abort();
    await this.operation;
    await this.releaseAll();
    this.listeners.clear();
  }
}
