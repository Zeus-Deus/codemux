import { randomUUID, type Host, type PluginContext, type PluginToolProgress, type PluginToolStatus } from "@codemux/sdk";

import {
  connectionFailure, controlFiles, controlInstructions, discoveryFiles, parseDoctor, parseVersion,
  readFrame, readWindows, toolText, viewFiles,
  CONTROL_TOOLS, DISCOVERY_TOOLS, CONTROL_LIFETIME_MS,
  type DoctorReport, type PreviewFrame, type WindowTarget,
} from "./model";

export interface ComputerUseServices {
  process: Host["process"];
  os: Host["os"];
  fs: Pick<Host["fs"], "dataDir" | "listDir" | "writeDataFile" | "removeDataFile">;
  tools: Pick<Host["tools"], "status" | "install">;
  mcp: Pick<Host["mcp"], "listTools" | "callTool" | "getStatus" | "onStatus" | "onActivity">;
  registerMcpServer: PluginContext["registerMcpServer"];
}

export interface Workspace {
  id: string;
  title: string;
  local: boolean;
}

export const DRIVER_TOOL = "cua-driver";
export const DRIVER_VERSION = "0.21.0";

/** Whether the driver can be used, separate from the plugin being on. */
export type DriverStatus = "unchecked" | "missing" | "incompatible" | "needs-setup" | "ready" | "error";

export interface DriverState {
  status: DriverStatus;
  /** Plain explanation for anything but `ready`. */
  message: string | null;
  /** Where the checked executable came from. */
  source: "custom" | "managed" | "system" | null;
}

export type InstallStage = PluginToolProgress | { stage: "checking" };

export interface ComputerUseState {
  workspace: Workspace | null;
  optedIn: boolean;
  driver: DriverState;
  /** Codemux-managed install support on this computer, once known. */
  managed: Pick<PluginToolStatus, "supported" | "unsupportedReason"> | null;
  install: InstallStage | null;
  installError: string | null;
  executable: string | null;
  version: string | null;
  doctor: DoctorReport | null;
  phase: "idle" | "detecting" | "installing" | "discovering" | "choosing" | "starting" | "active" | "stopping" | "expired" | "error";
  expiresAt: number | null;
  windows: WindowTarget[];
  target: WindowTarget | null;
  activeCalls: number;
  frame: PreviewFrame | null;
  error: string | null;
}

interface Resource {
  id: string;
  stop(): Promise<void>;
  files: string[];
  unsubscribe: (() => void)[];
  stopping?: Promise<void>;
}

class Cancelled extends Error {}

const DRIVER_ENV = { CUA_DRIVER_EMBEDDED: "1", CUA_DRIVER_RS_TELEMETRY_ENABLED: "0" };
type Candidate = { path: string; source: NonNullable<DriverState["source"]> };

const driverFile = /^cua-(?:discovery|view|control)-[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}\.(?:yaml|json)$/;
// Incarnations can overlap during reload. Keep pending writes and registrations
// protected until their disposer confirms exit; a new page revokes old owners.
const liveFiles = new Set<string>();

export class ComputerUseController {
  private state: ComputerUseState = {
    workspace: null, optedIn: false, driver: { status: "unchecked", message: null, source: null },
    managed: null, install: null, installError: null, executable: null, version: null, doctor: null,
    phase: "idle", expiresAt: null, windows: [], target: null, activeCalls: 0, frame: null, error: null,
  };
  private readonly listeners = new Set<() => void>();
  private readonly resources = new Set<Resource>();
  private readonly calls = new Set<string>();
  private abort = new AbortController();
  private operation: Promise<void> = Promise.resolve();
  private view: Resource | null = null;
  private previewBusy = false;
  private disposed = false;
  private expiryTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly services: ComputerUseServices) {}

  sweepStaleFiles(): Promise<void> {
    this.operation = this.operation.then(() => this.sweepFiles()).catch((error: unknown) => {
      if (!this.disposed) console.warn("[computer-use] Could not sweep stale driver files", error);
    });
    return this.operation;
  }

  private async sweepFiles(): Promise<void> {
    if (this.disposed) return;
    const directory = await this.services.fs.dataDir();
    const entries = await this.services.fs.listDir(directory);
    for (const entry of entries) {
      if (this.disposed) return;
      if (entry.kind !== "file" || !driverFile.test(entry.name) || liveFiles.has(entry.name)) continue;
      await this.services.fs.removeDataFile(entry.name);
      console.info("[computer-use] Removed stale driver file", entry.name);
    }
  }

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
    // Checking or installing the driver is machine-wide; a workspace switch
    // must not cancel it.
    if (this.resources.size === 0 && (this.state.phase === "detecting" || this.state.phase === "installing")) {
      return Promise.resolve();
    }
    if (this.resources.size === 0 && this.state.phase === "idle") {
      this.abort.abort();
      return Promise.resolve();
    }
    return this.stop();
  }

  /** Executables to try in order: PATH, then the official installers'
   *  standard locations, then Codemux's managed copy. */
  private async *candidates(signal: AbortSignal): AsyncGenerator<Candidate> {
    const onPath = await this.services.process.which("cua-driver");
    this.check(signal);
    if (onPath) yield { path: onPath, source: "system" };
    const os = await this.services.os.info();
    this.check(signal);
    const userBin = os.homeDir ? `${os.homeDir.replace(/\/+$/, "")}/.local/bin/cua-driver` : null;
    const standard: string[] = [];
    if (os.platform === "windows" && os.localDataDir) {
      standard.push(`${os.localDataDir.replace(/[\\/]+$/, "")}\\Programs\\Cua\\cua-driver\\bin\\cua-driver.exe`);
    } else if (os.platform === "macos") {
      standard.push("/Applications/CuaDriver.app/Contents/MacOS/cua-driver");
      if (userBin) standard.push(userBin);
    } else if (os.platform === "linux" && userBin) {
      standard.push(userBin);
    }
    for (const candidate of standard) {
      const executable = await this.services.process.which(candidate);
      this.check(signal);
      if (executable) yield { path: executable, source: "system" };
    }
    const managed = await this.managedStatus(signal);
    if (managed?.executable) yield { path: managed.executable, source: "managed" };
  }

  private async managedStatus(signal: AbortSignal): Promise<PluginToolStatus | null> {
    let status: PluginToolStatus | null = null;
    try {
      status = await this.services.tools.status(DRIVER_TOOL);
    } catch (error) {
      this.check(signal);
      this.update({ managed: { supported: false, unsupportedReason: `Codemux can’t install the driver here: ${error instanceof Error ? error.message : String(error)}` } });
      return null;
    }
    this.check(signal);
    this.update({ managed: { supported: status.supported, unsupportedReason: status.unsupportedReason } });
    return status;
  }

  /** The executable's version, or why it can't be used. */
  private async readVersion(executable: string, signal: AbortSignal): Promise<{ version: string | null; problem: string | null }> {
    let result: Awaited<ReturnType<ComputerUseServices["process"]["run"]>>;
    try {
      result = await this.services.process.run({
        command: executable, args: ["--version"], timeoutMs: 5_000, maxOutputBytes: 16_384, env: DRIVER_ENV,
      });
    } catch (error) {
      // A candidate that can't be launched is unusable, not fatal: detection
      // moves on to the next location. Cancellation still ends it.
      this.check(signal);
      return { version: null, problem: `Couldn’t run ${executable}: ${error instanceof Error ? error.message : String(error)}` };
    }
    this.check(signal);
    const version = parseVersion(result.stdout);
    if (result.code !== 0 || !version || result.cancelled || result.truncated || result.pipesHeldOpen || result.timedOut) {
      return { version, problem: result.stderr.trim() || "Could not read the cua-driver version." };
    }
    if (version !== DRIVER_VERSION) {
      return { version, problem: `Found cua-driver ${version}. Computer use needs version ${DRIVER_VERSION}, the version its safety limits were checked with.` };
    }
    return { version, problem: null };
  }

  /** Detection failures belong to the driver state, not the control error. */
  private async runDetection(command: string | undefined, signal: AbortSignal): Promise<void> {
    try {
      await this.detectDriver(command, signal);
    } catch (error) {
      if (error instanceof Cancelled || signal.aborted || this.disposed) throw error;
      this.update({ phase: "idle", driver: { status: "error", source: null, message: error instanceof Error ? error.message : String(error) } });
    }
  }

  private async detectDriver(command: string | undefined, signal: AbortSignal): Promise<void> {
    this.update({ phase: "detecting", error: null, executable: null, version: null, doctor: null });
    const requested = command?.trim();
    const custom = !!requested && requested !== "cua-driver";
    let chosen: (Candidate & { version: string }) | null = null;
    let found: { candidate: Candidate; version: string | null; problem: string } | null = null;
    if (custom) {
      const path = await this.services.process.which(requested);
      this.check(signal);
      if (!path) {
        this.update({ phase: "idle", driver: { status: "missing", source: "custom",
          message: `No executable was found at ${requested}. Check the path, or use automatic detection.` } });
        if (!this.state.managed) await this.managedStatus(signal);
        return;
      }
      const { version, problem } = await this.readVersion(path, signal);
      if (problem) found = { candidate: { path, source: "custom" }, version, problem };
      else if (version) chosen = { path, source: "custom", version };
    } else {
      const seen = new Set<string>();
      for await (const candidate of this.candidates(signal)) {
        if (seen.has(candidate.path)) continue;
        seen.add(candidate.path);
        const { version, problem } = await this.readVersion(candidate.path, signal);
        if (!problem && version) { chosen = { ...candidate, version }; break; }
        found ??= { candidate, version, problem: problem ?? "Could not read the cua-driver version." };
      }
    }
    if (!chosen) {
      if (custom || found) {
        this.update({ phase: "idle", executable: found?.candidate.path ?? null, version: found?.version ?? null,
          driver: { status: "incompatible", source: found?.candidate.source ?? "custom", message: found?.problem ?? null } });
      } else {
        this.update({ phase: "idle", driver: { status: "missing", source: null, message: null } });
      }
      if (!this.state.managed) await this.managedStatus(signal);
      return;
    }
    const { path: executable, version, source } = chosen;
    this.update({ executable, version });
    const result = await this.services.process.run({
      command: executable, args: ["doctor", "--json"], timeoutMs: 15_000, maxOutputBytes: 262_144, env: DRIVER_ENV,
    });
    this.check(signal);
    const doctor = parseDoctor(result.stdout);
    if (!doctor || result.cancelled || result.truncated || result.pipesHeldOpen || result.timedOut) {
      this.update({ phase: "idle", driver: { status: "error", source,
        message: result.stderr.trim() || "cua-driver did not return a complete diagnostic report." } });
      return;
    }
    if (result.code !== 0 || !doctor.ok) {
      this.update({ phase: "idle", doctor: { ...doctor, ok: false }, driver: { status: "needs-setup", source,
        message: result.code !== 0
          ? result.stderr.trim() || `cua-driver doctor exited with code ${result.code ?? "unknown"}. Review its diagnostics.`
          : "The driver is installed but isn’t ready yet." } });
      return;
    }
    this.update({ phase: "idle", doctor, driver: { status: "ready", source, message: null } });
  }

  private canCheck() {
    return this.resources.size === 0 && ["idle", "expired", "error"].includes(this.state.phase);
  }

  /** Check the driver. Pass the saved custom path, if any. */
  detect(command?: string): Promise<void> {
    return this.schedule(async (signal) => {
      await this.releaseAll();
      this.check(signal);
      await this.runDetection(command, signal);
    });
  }

  /** Re-check on opening setup. Runs only from a plain idle state, so it
   *  never disturbs control, a check under way, or an expiry/error notice.
   *  With `onlyOnce`, skip a driver already checked. */
  refresh(command?: string, onlyOnce = false): Promise<void> {
    if (this.resources.size > 0 || this.state.phase !== "idle" || (onlyOnce && this.state.driver.status !== "unchecked")) {
      return Promise.resolve();
    }
    this.update({ phase: "detecting" });
    return this.detect(command);
  }

  /** Install the pinned driver into Codemux's data folder, then check it.
   *  This never opts a workspace in, chooses a window or starts control. */
  install(): Promise<void> {
    if (!this.canCheck()) return Promise.resolve();
    this.update({ phase: "installing", install: { stage: "downloading", received: 0, total: 0 }, installError: null, error: null });
    return this.schedule(async (signal) => {
      await this.releaseAll();
      this.check(signal);
      try {
        await this.services.tools.install(DRIVER_TOOL, (progress) => {
          if (!signal.aborted && !this.disposed) this.update({ install: progress });
        });
      } catch (error) {
        this.check(signal);
        this.update({ phase: "idle", install: null, installError: error instanceof Error ? error.message : String(error) });
        return;
      }
      this.check(signal);
      this.update({ install: { stage: "checking" } });
      await this.runDetection(undefined, signal);
      this.update({ install: null });
    });
  }

  private authorized() {
    const { workspace, optedIn, executable, driver } = this.state;
    if (!workspace || !optedIn) throw new Error("Allow computer use in this workspace first.");
    if (!workspace.local) throw new Error("Computer use is available only in local workspaces.");
    if (!executable || driver.status === "unchecked") throw new Error("Check the cua-driver installation first.");
    if (driver.status !== "ready" || this.state.doctor?.ok !== true) throw new Error("Finish the driver setup before enabling computer use.");
    return { workspace, executable };
  }

  private async open(
    kind: "discovery" | "view" | "control",
    files: { policy: string; manifest: string },
    signal: AbortSignal,
    target?: WindowTarget,
  ): Promise<Resource> {
    const { workspace, executable } = this.authorized();
    await this.sweepFiles();
    this.check(signal);
    const id = `cua-${kind}-${randomUUID()}`;
    const names = [`${id}.yaml`, `${id}.json`];
    for (const name of names) liveFiles.add(name);
    let resource: Resource | null = null;
    try {
      const policy = await this.services.fs.writeDataFile(names[0], files.policy);
      this.check(signal);
      const manifest = await this.services.fs.writeDataFile(names[1], files.manifest);
      this.check(signal);
      // Probe the exact files without agent visibility before registering input.
      for (const probe of kind === "control" ? [true, false] : [false]) {
        this.check(signal);
        const localId = probe ? `${id}-probe` : id;
        const stop = await this.services.registerMcpServer({
          id: localId, name: probe ? "Computer use policy check" : kind === "control" ? "Computer use" : "Computer use preview",
          scope: kind === "control" && !probe ? { workspaceIds: [workspace.id] } : "plugin",
          toolPrefix: probe ? "computer-use-probe" : kind === "control" ? "computer-use" : `computer-use-${kind}`,
          ...(kind === "control" && target ? {
            toolDescriptions: Object.fromEntries(CONTROL_TOOLS.map((tool) => [tool, { append: controlInstructions(target) }])),
          } : {}),
          shutdown: "immediate",
          transport: "stdio", command: executable, args: ["mcp", "--direct", "--no-overlay"],
          env: {
            CUA_DRIVER_PERMISSION_MODE: "bounded", CUA_DRIVER_CAPABILITY_MANIFEST_FILE: manifest,
            CUA_DRIVER_CAPABILITY_MANIFEST_APPROVED: "1", CUA_DRIVER_POLICY_FILE: policy,
            CUA_DRIVER_RS_TELEMETRY_ENABLED: "0", CUA_DRIVER_EMBEDDED: "1",
          },
        });
        resource = { id: localId, stop, files: names, unsubscribe: [] };
        this.resources.add(resource);
        this.check(signal);
        const deadline = Date.now() + 15_000;
        while (Date.now() < deadline) {
          this.check(signal);
          const runtime = await this.services.mcp.getStatus(localId);
          this.check(signal);
          if (runtime?.status.kind === "running") {
            const allowed: readonly string[] = kind === "control" ? CONTROL_TOOLS : kind === "view" ? ["get_window_state"] : DISCOVERY_TOOLS;
            const tools = await this.services.mcp.listTools(localId);
            this.check(signal);
            const extra = tools.filter((tool) => !allowed.includes(tool.name));
            if (extra.length) throw new Error(`cua-driver policy was not applied: unexpected tools: ${extra.map((tool) => tool.name).join(", ")}`);
            if (!tools.some((tool) => tool.name === (kind === "discovery" ? "list_windows" : "get_window_state"))) {
              throw new Error("cua-driver policy check returned no window observation tool.");
            }
            break;
          }
          if (runtime?.status.kind === "errored") throw new Error(runtime.status.message);
          if (runtime?.status.kind === "stopped") throw new Error("cua-driver stopped during startup.");
          await this.delay(signal);
        }
        if (Date.now() >= deadline) throw new Error("cua-driver did not become ready.");
        if (!probe) return resource;
        await this.release(resource, true);
        resource = null;
      }
      throw new Error("cua-driver did not become ready.");
    } catch (error) {
      if (resource) await this.release(resource);
      else {
        for (const name of names) liveFiles.delete(name);
        await Promise.all(names.map((name) => this.services.fs.removeDataFile(name).catch(() => {})));
      }
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
      const diagnostic = await this.services.mcp.callTool(discovery.id, "get_screen_size");
      this.check(signal);
      const failure = connectionFailure(diagnostic);
      if (failure) throw new Error(failure);
      const result = await this.services.mcp.callTool(discovery.id, "list_windows");
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
      const expiresAt = Date.now() + CONTROL_LIFETIME_MS;
      this.expiryTimer = setTimeout(() => { void this.stop(true); }, CONTROL_LIFETIME_MS);
      this.update({ phase: "starting", target, expiresAt, frame: null, error: null });
      // Polling in a separate read-only runtime leaves the agent's element tokens intact.
      const view = await this.open("view", viewFiles(target), signal, target);
      const args = { pid: target.pid, window_id: target.windowId, include_screenshot: true };
      const snapshot = await this.services.mcp.callTool(view.id, "get_window_state", args);
      this.check(signal);
      if (snapshot.isError) throw new Error(toolText(snapshot) ?? "The selected window is no longer available.");
      const control = await this.open("control", controlFiles(target), signal, target);
      control.unsubscribe.push(this.services.mcp.onActivity(control.id, (event) => {
        if (this.abort.signal !== signal || event.workspaceId !== workspace.id) return;
        if (event.phase === "start") this.calls.add(event.callId);
        else this.calls.delete(event.callId);
        this.update({ activeCalls: this.calls.size });
      }));
      control.unsubscribe.push(this.services.mcp.onStatus(control.id, (runtime) => {
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
      const result = await this.services.mcp.callTool(view.id, "get_window_state", {
        pid: target.pid, window_id: target.windowId, include_screenshot: true,
      });
      if (!signal.aborted && this.view === view) this.update({ frame: readFrame(result) });
    } catch (error) {
      if (!signal.aborted && this.view === view) this.update({ frame: { kind: "unavailable", message: error instanceof Error ? error.message : String(error) } });
    } finally { this.previewBusy = false; }
  }

  private release(resource: Resource, retainFiles = false): Promise<void> {
    resource.stopping ??= (async () => {
      await resource.stop();
      for (const unsubscribe of resource.unsubscribe) unsubscribe();
      this.resources.delete(resource);
      if (this.view === resource) this.view = null;
      if (!retainFiles) {
        for (const name of resource.files) liveFiles.delete(name);
        await Promise.all(resource.files.map((name) => this.services.fs.removeDataFile(name).catch(() => {})));
      }
    })().catch((error: unknown) => { resource.stopping = undefined; throw error; });
    return resource.stopping;
  }

  private async releaseAll() {
    if (this.expiryTimer !== null) clearTimeout(this.expiryTimer);
    this.expiryTimer = null;
    await Promise.all([...this.resources].map((resource) => this.release(resource)));
    this.calls.clear();
    this.update({ activeCalls: 0 });
  }

  stop(expired = false): Promise<void> {
    this.update({ phase: "stopping" });
    const immediate = this.releaseAll();
    // Observe a failure now, then surface it through the serialized operation.
    void immediate.catch(() => {});
    return this.schedule(async () => {
      await immediate;
      await this.releaseAll();
      this.update({ phase: expired ? "expired" : "idle", expiresAt: null, windows: [], activeCalls: 0, frame: null, target: null,
        error: expired ? "Computer use expired after 10 minutes. Choose an application window again." : null });
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
