#!/usr/bin/env node
// Headless browser driver for a live root, on top of `agent-browser`
// (headless Chromium; no X server involved).
//
// Library:  import { Driver, loadEnv } from "./drive.mjs"
// CLI:      node drive.mjs <command> [args…]   (root: $ORCH_LIVE_ROOT or newest)
//   pair                       open a fresh pairing URL and wait for the app
//   invoke <cmd> [json-args]   call a backend command through the remote transport
//   eval <js>                  evaluate JS in the page, print the JSON result
//   click <testid>             click [data-testid=<testid>]
//   wait <js> [timeout-ms]     poll a JS expression until it is truthy
//   shot <name>                screenshot at desktop width and 390×844
//   close                      close the browser session
import { execFile } from "node:child_process";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { promisify } from "node:util";

const run = promisify(execFile);
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

export const DESKTOP = { width: 1440, height: 900 };
export const PHONE = { width: 390, height: 844 };

/** Parse `<root>/env` (KEY='value' lines written by up.sh). */
export function loadEnv(rootArg = process.env.ORCH_LIVE_ROOT) {
  let root = rootArg;
  if (!root) {
    const base = process.env.ORCH_LIVE_BASE ?? join(homedir(), ".cache/codemux-agents/orch-v2/live");
    const runs = existsSync(base) ? readdirSync(base).filter((name) => /^2\d+/.test(name)).sort() : [];
    if (!runs.length) throw new Error(`no live root under ${base}; run up.sh`);
    root = join(base, runs[runs.length - 1]);
  }
  const file = join(resolve(root), "env");
  if (!existsSync(file)) throw new Error(`${file} missing; run up.sh`);
  const env = {};
  for (const line of readFileSync(file, "utf8").split("\n")) {
    const match = /^([A-Z_]+)='(.*)'$/.exec(line.trim());
    if (match) env[match[1]] = match[2];
  }
  return env;
}

/** `codemux json <cmd> <args>` against the isolated instance (see lib.sh cmx). */
export async function codemux(env, cmd, args = {}) {
  const { stdout } = await run(env.BIN, ["json", cmd, JSON.stringify(args)], {
    env: {
      HOME: process.env.HOME,
      PATH: "/usr/bin:/bin",
      XDG_CONFIG_HOME: `${env.ROOT}/config`,
      XDG_DATA_HOME: `${env.ROOT}/data`,
      XDG_CACHE_HOME: `${env.ROOT}/cache`,
      XDG_STATE_HOME: `${env.ROOT}/state`,
      XDG_RUNTIME_DIR: `${env.ROOT}/runtime`,
    },
    maxBuffer: 64 << 20,
  });
  const reply = JSON.parse(stdout);
  if (reply && reply.ok === false) throw new Error(`${cmd}: ${reply.error}`);
  return reply && "data" in reply ? reply.data : reply;
}

export class Driver {
  constructor(env) {
    this.env = env;
    this.session = env.BROWSER_SESSION;
    this.shotsDir = join(env.ROOT, "shots");
    this.viewport = null;
  }

  async ab(args, { json = true } = {}) {
    const argv = ["--session", this.session, ...(json ? ["--json"] : []), ...args];
    let stdout;
    try {
      ({ stdout } = await run("agent-browser", argv, { maxBuffer: 64 << 20, timeout: 120_000 }));
    } catch (error) {
      stdout = error.stdout;
      if (!stdout) throw new Error(`agent-browser ${args[0]} failed: ${error.message}`);
    }
    if (!json) return stdout;
    const reply = JSON.parse(stdout);
    if (!reply.success) throw new Error(`agent-browser ${args[0]}: ${reply.error ?? stdout}`);
    return reply.data;
  }

  async setViewport({ width, height }) {
    if (this.viewport?.width === width && this.viewport?.height === height) return;
    await this.ab(["set", "viewport", String(width), String(height)]);
    this.viewport = { width, height };
  }

  /** Evaluate an expression (promises are awaited) and return its value. */
  async eval(js) {
    return (await this.ab(["eval", js])).result;
  }

  /** Mint a fresh pairing URL (tokens are single use), open it and wait for
   *  the paired app shell, past the sign-in screen. */
  async pair({ timeout = 60_000 } = {}) {
    const pairing = await codemux(this.env, "web_remote_pair", { name: "orch-live" });
    await this.setViewport(DESKTOP);
    // The minted URL names the preferred endpoint; serve listens on loopback.
    await this.ab(["open", `${this.env.BASE_URL}/#pair=${pairing.token}`]);
    await this.waitFor("window.__CODEMUX_REMOTE__ === true && !!window.__TAURI_INTERNALS__", {
      timeout, label: "remote shim installed",
    });
    await this.waitFor(
      `!document.querySelector('[data-testid="login-view"]') && !!document.querySelector('[data-testid="workspace-content-surface"], [data-testid="home-landing"]')`,
      { timeout, label: "app shell past sign-in" },
    );
  }

  /** Call a backend command through the page's remote transport — the same
   *  path every UI action takes, including the web-remote allow-list. */
  async invoke(cmd, args = {}) {
    const reply = await this.eval(`(async () => {
      try { return { ok: true, value: await window.__TAURI_INTERNALS__.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(args)}) }; }
      catch (error) { return { ok: false, error: typeof error === "string" ? error : (error && error.message) || JSON.stringify(error) }; }
    })()`);
    if (!reply?.ok) throw new Error(`invoke ${cmd}: ${reply?.error}`);
    return reply.value;
  }

  /** A real click on a data-testid, so the UI path is what gets tested. */
  async click(testid) {
    await this.clickSelector(`[data-testid="${testid}"]`);
  }

  async clickSelector(selector) {
    // An overflowing strip (e.g. many tabs) can hide the target; a click at
    // its coordinates would land on whatever covers them.
    await this.ab(["scrollintoview", selector]).catch(() => {});
    await this.ab(["click", selector]);
  }

  /** Poll a page expression (or an async node function) until truthy. */
  async waitFor(check, { timeout = 600_000, interval = 1_000, label } = {}) {
    const deadline = Date.now() + timeout;
    let last;
    for (;;) {
      try {
        last = typeof check === "function" ? await check() : await this.eval(check);
        if (last) return last;
      } catch (error) {
        last = error.message;
      }
      if (Date.now() > deadline) {
        throw new Error(`timed out after ${timeout} ms waiting for ${label ?? check} (last: ${JSON.stringify(last)})`);
      }
      await sleep(interval);
    }
  }

  /** Screenshot at desktop width and at 390×844; returns both paths. Below
   *  the phone breakpoint the app swaps to the phone shell, which opens on
   *  its workspace list: `phone(driver)` navigates to what the shot should
   *  show before it is taken. */
  async shot(name, { phone } = {}) {
    const paths = {};
    for (const [kind, size] of [["desktop", DESKTOP], ["mobile", PHONE]]) {
      await this.setViewport(size);
      await sleep(600); // let layout settle after the resize
      if (kind === "mobile" && phone) {
        await phone(this);
        await sleep(600);
      }
      paths[kind] = join(this.shotsDir, `${name}-${kind}.png`);
      await this.ab(["screenshot", paths[kind]]);
    }
    await this.setViewport(DESKTOP);
    return paths;
  }

  async close() {
    await this.ab(["close"], { json: false }).catch(() => {});
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [cmd, ...rest] = process.argv.slice(2);
  const driver = new Driver(loadEnv());
  const out = (value) => console.log(JSON.stringify(value, null, 2));
  const commands = {
    pair: () => driver.pair(),
    invoke: () => driver.invoke(rest[0], rest[1] ? JSON.parse(rest[1]) : {}),
    eval: () => driver.eval(rest[0]),
    click: () => driver.click(rest[0]),
    wait: () => driver.waitFor(rest[0], { timeout: Number(rest[1] ?? 60_000) }),
    shot: () => driver.shot(rest[0] ?? "manual"),
    close: () => driver.close(),
  };
  if (!commands[cmd]) {
    console.error(readFileSync(new URL(import.meta.url), "utf8").split("\n").slice(1, 13).join("\n"));
    process.exit(2);
  }
  commands[cmd]().then(out, (error) => { console.error(`FAIL: ${error.message}`); process.exit(1); });
}
