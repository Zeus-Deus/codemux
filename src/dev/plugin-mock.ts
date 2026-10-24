/**
 * Dev-mock handlers for every `plugins_*` command. The mock has no file
 * system: the plugins folder is empty unless `?plugins=hello` seeds one disk
 * plugin from source, which exercises the real blob loader in a browser.
 */
type Args = Record<string, unknown>;
type Handler = (args: Args) => unknown;

const HELLO = `import { host, PALETTE_AREA } from "@codemux/sdk";
export default {
  id: "hello",
  name: "Hello",
  description: "Mock disk plugin",
  version: "0.1.0",
  register(ctx) {
    ctx.register({ id: "say", area: PALETTE_AREA, data: {
      label: "Say hello", run: () => host.notify({ message: "Hello from a mock disk plugin" }) } });
  },
};
`;

/**
 * Computer use driver fixtures, chosen with `?computerUse=`: `missing`
 * (default), `ready`, `incompatible`, `setup` (an OS permission is missing),
 * `failed` (the first install download fails) or `unsupported` (no managed
 * install on this platform). Paths and versions are example data.
 */
function computerUseMock(push: (channel: unknown, message: unknown) => void): Record<string, Handler> {
  const scenario = new URLSearchParams(location.search).get("computerUse") ?? "missing";
  const managed = "/mock/codemux-dev/tools/cua-driver/0.21.0-linux-x86_64/cua-driver";
  let installed = scenario === "ready";
  let attempts = 0;
  const system = scenario === "incompatible" || scenario === "setup" ? "/usr/local/bin/cua-driver" : null;
  const result = (stdout: string, code = 0) => ({ code, stdout, stderr: "", timedOut: false, truncated: false, pipesHeldOpen: false, cancelled: false });
  const healthy = { ok: true, probes: [{ label: "display server", status: "ok", message: "X11 session available." }] };
  const permission = { ok: false, probes: [
    { label: "display server", status: "ok", message: "X11 session available." },
    { label: "screen capture", status: "error", message: "Screen capture permission is not granted.", detail: "Allow screen capture for Codemux, then check again." },
  ] };
  return {
    plugins_host_acquire: () => null,
    plugins_host_release: () => null,
    plugins_os_info: () => ({ platform: "linux", arch: "x86_64", homeDir: "/mock/home", configDir: null, dataDir: null, localDataDir: null }),
    plugins_process_which: (a) => {
      const command = String(a.command);
      if (command === "cua-driver") return system;
      return command === managed && installed ? managed : null;
    },
    plugins_process_run: async (a) => {
      const input = a.input as { command: string; args?: string[] };
      await new Promise((resolve) => setTimeout(resolve, 250));
      if (input.args?.[0] === "--version") return result(`cua-driver ${input.command === system && scenario === "incompatible" ? "0.19.2" : "0.21.0"}\n`);
      return result(JSON.stringify(scenario === "setup" ? permission : healthy));
    },
    plugins_fs: (a) => {
      const operation = a.operation as { kind: string; name?: string };
      if (operation.kind === "dataDir") return "/mock/codemux-dev/plugin-data/computer-use";
      if (operation.kind === "listDir") return [];
      if (operation.kind === "writeDataFile") return `/mock/codemux-dev/plugin-data/computer-use/${operation.name}`;
      return null;
    },
    plugins_tool_status: () => ({
      id: "cua-driver", version: "0.21.0", supported: scenario !== "unsupported",
      unsupportedReason: scenario === "unsupported" ? "Codemux can’t install the driver on macOS. macOS permissions need the driver app in /Applications, which the official installer sets up." : null,
      executable: installed ? managed : null,
    }),
    plugins_tool_install: async (a) => {
      const total = 28_809_630;
      const failing = scenario === "failed" && ++attempts === 1;
      for (let i = 0; i <= 10; i++) {
        push(a.onProgress, { stage: "downloading", received: Math.round((total * i) / 10), total });
        await new Promise((resolve) => setTimeout(resolve, 180));
        if (failing && i === 6) throw "The download failed: HTTP 503. Check your connection and try again.";
      }
      push(a.onProgress, { stage: "verifying" });
      await new Promise((resolve) => setTimeout(resolve, 500));
      push(a.onProgress, { stage: "validating" });
      await new Promise((resolve) => setTimeout(resolve, 500));
      installed = true;
      return managed;
    },
  };
}

export function pluginMockHandlers(push: (channel: unknown, message: unknown) => void = () => {}): Record<string, Handler> {
  const seeded = new URLSearchParams(location.search).get("plugins") === "hello";
  const root = "/mock/codemux-dev/plugins";
  const decisions: Record<string, boolean> = {};
  const disk: Record<string, string> = seeded ? { hello: HELLO } : {};
  return {
    ...computerUseMock(push),
    plugins_root: () => ({ path: root, disabledByEnv: false }),
    plugins_list: () =>
      Object.keys(disk)
        .sort()
        .map((id) => ({ id, file: `${root}/${id}/plugin.js`, dir: `${root}/${id}`, marker: null, error: null })),
    plugins_read_source: (a) => {
      const source = disk[String(a.id)];
      if (source === undefined) throw new Error(`Plugin "${String(a.id)}" is not installed`);
      return source;
    },
    plugins_watch: () => null,
    plugins_reveal_root: () => null,
    plugins_get_decisions: () => ({ ...decisions }),
    plugins_set_enabled: (a) => {
      decisions[String(a.id)] = a.on === true;
      return null;
    },
    plugins_safe_mode_status: () => ({ paused: false, reason: null, disabledByEnv: false }),
    plugins_startup_checkpoint: () => null,
    plugins_resume: () => null,
    plugins_set_event_tap: () => null,
    // No live sessions in the mock: every thread reads as having none.
    plugins_chat_status: () => null,
    plugins_chat_request_compaction: () => ({ started: false, reason: "no_session" }),
    plugins_catalog: () => ({
      catalog: { schemaVersion: 1, revision: 1, plugins: [], blocked: [] },
      fetchedAt: null,
      error: null,
    }),
    plugins_preview_install: () => {
      throw new Error("Installing plugins needs the desktop app");
    },
    plugins_install: () => {
      throw new Error("Installing plugins needs the desktop app");
    },
    plugins_remove: (a) => {
      delete disk[String(a.id)];
      delete decisions[String(a.id)];
      return null;
    },
  };
}
