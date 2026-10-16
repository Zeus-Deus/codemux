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

export function pluginMockHandlers(): Record<string, Handler> {
  const seeded = new URLSearchParams(location.search).get("plugins") === "hello";
  const root = "/mock/codemux-dev/plugins";
  const decisions: Record<string, boolean> = {};
  const disk: Record<string, string> = seeded ? { hello: HELLO } : {};
  return {
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
