#!/usr/bin/env node
// Run live scenarios against the root up.sh brought up and write
// <root>/report.md (+ report.json).
//
//   node run.mjs [scenario…]        default: every scenarios/*.mjs
//
// A scenario module exports `name`, `description` and `async run(ctx)`; see
// README.md for the ctx API. Each scenario gets a 10-minute budget
// (ORCH_LIVE_SCENARIO_TIMEOUT_MS overrides it). When it runs out, the
// scenario's signal aborts: in-flight browser, CLI and assert calls are killed
// and every wait throws. A scenario that still hasn't stopped after a grace
// period ends the whole run, so it can't keep driving the browser under the
// next one.
import { execFile } from "node:child_process";
import { randomUUID } from "node:crypto";
import { readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { setTimeout as sleep } from "node:timers/promises";
import { promisify } from "node:util";
import { Driver, codemux, loadEnv } from "./drive.mjs";

const run = promisify(execFile);
const here = dirname(fileURLToPath(import.meta.url));
const env = loadEnv();
const driver = new Driver(env);
const budget = Number(process.env.ORCH_LIVE_SCENARIO_TIMEOUT_MS ?? 600_000);
const abortGrace = 30_000;

/** assert.sh wrappers. Each resolves to the parsed evidence or throws. */
const assert = {
  async call(...args) {
    try {
      const { stdout } = await run(join(here, "assert.sh"), args, {
        env: { ...process.env, ORCH_LIVE_ROOT: env.ROOT },
        maxBuffer: 64 << 20,
        signal: driver.signal ?? undefined,
      });
      return JSON.parse(stdout || "null");
    } catch (error) {
      driver.signal?.throwIfAborted();
      throw new Error(`assert.sh ${args[0]}: ${(error.stderr || error.message).trim()}`);
    }
  },
  sql: (query) => assert.call("sql", query).then((rows) => rows ?? []),
  conversation: (threadId, limit = 50) => assert.call("conversation", threadId, String(limit)),
  reply: (threadId, regex) => assert.call("reply", threadId, regex),
  noText: (text) => assert.call("no-text", text),
};

function context(result, signal) {
  const ctx = {
    env, driver, assert,
    /** Aborts when the scenario's budget runs out; pass it to anything
     *  long-running the scenario starts itself. */
    signal,
    codemux: (cmd, args) => codemux(env, cmd, args, { signal }),
    /** Record a named check; a falsy `ok` fails the scenario. */
    check(label, ok, evidence) {
      result.checks.push({ label, ok: !!ok, evidence });
      if (!ok) throw new Error(`check failed: ${label}${evidence ? ` (${JSON.stringify(evidence)})` : ""}`);
    },
    /** Record an observed id or value for the report. */
    observe(key, value) { result.observed[key] = value; },
    async shot(name, opts) {
      const paths = await driver.shot(name, opts);
      result.shots.push(paths.desktop, paths.mobile);
      return paths;
    },
    waitFor: (check, opts) => driver.waitFor(check, opts),

    /** Start a chat the way the composer's first send does: a pane bound to
     *  a pre-minted thread, a provider session, then the first turn. Every
     *  call goes through the paired browser's remote transport. Returns
     *  { threadId, paneId, tabId }. */
    async startChat({ provider, model = null, effort = null, text, permissionMode = null, extra = null }) {
      const threadId = randomUUID();
      const paneId = await driver.invoke("agent_chat_create_pane", {
        workspaceId: env.WORKSPACE_ID, provider, cwd: env.REPO,
        launchMode: "new_tab", threadId,
      });
      await driver.invoke("agent_chat_start_session", {
        paneId, provider, expectedThread: null,
        input: {
          thread_id: threadId, cwd: env.REPO, model, resume_cursor: null,
          permission_mode: permissionMode, effort, context_window: null,
          service_tier: "default", additional_directories: [], env: null, extra,
        },
      });
      await driver.invoke("agent_chat_send_turn", {
        provider,
        input: {
          thread_id: threadId, text, display_text: text, skill_ids: [],
          skill_text: null, include_plugins: true, images: [],
          model_override: null, effort_override: effort, permission_mode_override: null,
        },
      });
      return { threadId, paneId, tabId: await ctx.tabOf(paneId) };
    },

    /** The tab holding a pane, from the backend's app state. */
    async tabOf(paneId) {
      const state = await codemux(env, "get_app_state", {}, { signal });
      const workspace = state.workspaces.find((w) => w.workspace_id === env.WORKSPACE_ID);
      const contains = (node) => !!node && (node.pane_id === paneId
        || Object.values(node).some((child) => child && typeof child === "object" && contains(child)));
      const surface = workspace?.surfaces.find((s) => contains(s.root));
      return workspace?.tabs.find((t) => t.surface_id === surface?.surface_id)?.tab_id ?? null;
    },

    /** Show a tab the way a user does: click it in the tab strip. */
    async openTab(tabId) {
      await driver.waitFor(`!!document.querySelector('[data-tab-id="${tabId}"]')`, {
        timeout: 30_000, label: `tab ${tabId} rendered`,
      });
      await driver.clickSelector(`[data-tab-id="${tabId}"]`);
    },
  };
  return ctx;
}

const dir = join(here, "scenarios");
const wanted = process.argv.slice(2);
const files = readdirSync(dir).filter((f) => f.endsWith(".mjs")).sort()
  .filter((f) => !wanted.length || wanted.includes(f.replace(/\.mjs$/, "")));
if (!files.length) throw new Error(`no scenarios match ${wanted.join(", ")}`);

const startedAt = new Date();
const results = [];
let runAborted = null;
await driver.pair();
for (const file of files) {
  const scenario = await import(pathToFileURL(join(dir, file)).href);
  const result = { name: scenario.name, description: scenario.description, ok: false,
    checks: [], observed: {}, shots: [], error: null, seconds: 0 };
  results.push(result);
  const t0 = Date.now();
  console.log(`== ${scenario.name}`);
  const controller = new AbortController();
  driver.signal = controller.signal;
  const timer = setTimeout(
    () => controller.abort(new Error(`scenario budget ${budget} ms exceeded`)), budget);
  const running = Promise.resolve().then(() => scenario.run(context(result, controller.signal)));
  const settled = running.then(() => true, () => true);
  // Stop waiting at the abort even if the scenario never looks at its signal.
  const aborted = new Promise((_, reject) => {
    controller.signal.addEventListener("abort", () => reject(controller.signal.reason), { once: true });
  });
  aborted.catch(() => {});
  try {
    await Promise.race([running, aborted]);
    result.ok = true;
  } catch (error) {
    result.error = controller.signal.aborted ? controller.signal.reason.message : error.message;
  } finally {
    clearTimeout(timer);
  }
  if (!result.ok) {
    // A scenario that ignores the abort is still holding the browser.
    controller.abort(new Error("scenario failed"));
    const stopped = await Promise.race([settled, sleep(abortGrace).then(() => false)]);
    driver.signal = null;
    if (!stopped) {
      runAborted = `${scenario.name} did not stop within ${abortGrace} ms of its abort; run aborted`;
    } else {
      // Leave evidence of where it stopped.
      await driver.shot(`${scenario.name}-failure`).then((p) => result.shots.push(p.desktop, p.mobile)).catch(() => {});
    }
  }
  driver.signal = null;
  result.seconds = Math.round((Date.now() - t0) / 1000);
  for (const check of result.checks) console.log(`  ${check.ok ? "PASS" : "FAIL"}: ${check.label}`);
  console.log(result.ok ? `PASS: ${scenario.name} (${result.seconds}s)` : `FAIL: ${scenario.name}: ${result.error}`);
  if (runAborted) {
    console.log(`FAIL: ${runAborted}`);
    break;
  }
}

const head = await run("git", ["-C", env.REPO_DIR, "rev-parse", "--short", "HEAD"]).then((r) => r.stdout.trim()).catch(() => "?");
const lines = [
  `# Orchestration live report`, "",
  `- Root: \`${env.ROOT}\``,
  `- Started: ${startedAt.toISOString()}`,
  `- Source: \`${env.REPO_DIR}\` at ${head}`,
  `- Binary: \`${env.BIN}\` (debug)`,
  `- Serve: ${env.BASE_URL} (pid ${env.SERVE_PID}), workspace \`${env.WORKSPACE_ID}\` at \`${env.REPO}\``,
  `- Paired browser: agent-browser session \`${env.BROWSER_SESSION}\`, local mode via \`CODEMUX_E2E_LOCAL_REMOTE=1\` (no source patch)`,
  ...(runAborted ? ["", `**Run aborted:** ${runAborted}`] : []),
  "",
  "| Scenario | Result | Time | Checks |", "|---|---|---|---|",
  ...results.map((r) => `| ${r.name} | ${r.ok ? "PASS" : "FAIL"} | ${r.seconds}s | ${r.checks.filter((c) => c.ok).length}/${r.checks.length} |`),
  "",
];
for (const r of results) {
  lines.push(`## ${r.name}`, "", r.description ?? "", "");
  if (r.error) lines.push(`**Error:** ${r.error}`, "");
  for (const c of r.checks) {
    lines.push(`- ${c.ok ? "PASS" : "FAIL"} ${c.label}${c.evidence === undefined ? "" : `: \`${JSON.stringify(c.evidence).slice(0, 300)}\``}`);
  }
  if (Object.keys(r.observed).length) {
    lines.push("", "Observed:", ...Object.entries(r.observed).map(([k, v]) => `- ${k}: \`${typeof v === "string" ? v : JSON.stringify(v)}\``));
  }
  if (r.shots.length) lines.push("", "Screenshots:", ...r.shots.map((p) => `- \`${p}\``));
  lines.push("");
}
writeFileSync(join(env.ROOT, "report.md"), lines.join("\n"));
writeFileSync(join(env.ROOT, "report.json"), JSON.stringify({ startedAt, runAborted, results }, null, 2));
console.log(`report: ${join(env.ROOT, "report.md")}`);
// exit() rather than returning: a runaway scenario must not keep going.
process.exit(!runAborted && results.every((r) => r.ok) ? 0 : 1);
