// The local setup commands Settings > Outside assistants prints (the guide
// prints the same, checked by its drift test) work as printed: each one, run
// in the sandbox with a throwaway HOME, registers a codemux server its
// client can reach and list tools from. Credential-free: the clients only
// edit their own config and start `codemux mcp`; nothing signs in.
//
// The commands come from the Settings source itself
// (src/components/settings/outside-assistants-setup.ts, imported with Node's
// type stripping), with the root's debug binary as the executable. Hermes
// runs only when ORCH_LIVE_PROVIDERS includes hermes, the only time up.sh
// mounts its runtime.
import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, realpathSync, symlinkSync } from "node:fs";
import { join } from "node:path";
import { StdioMcpClient, sandboxRun } from "../mcp-client.mjs";
import { localSetupCommands } from "../../../../src/components/settings/outside-assistants-setup.ts";

export const name = "outside-guide-setup";
export const description =
  "Run the four local setup commands from Settings > Outside assistants as printed, in the sandbox with a throwaway HOME: " +
  "Claude Code reports codemux connected, Codex lists it, Hermes lists thread_launch, and the bare command serves MCP.";

const read = (path) => (existsSync(path) ? readFileSync(path, "utf8") : "");

export async function run(ctx) {
  const { env, signal } = ctx;
  const tag = randomUUID().slice(0, 8);
  const status = await ctx.codemux("agent_connector_status", {});
  ctx.observe("Settings' local executable", { localExecutable: status.localExecutable ?? null, localCommand: status.localCommand });
  if (status.localExecutable?.startsWith("/")) {
    ctx.check("Settings names the binary this Codemux runs",
      existsSync(status.localExecutable) && realpathSync(status.localExecutable) === realpathSync(env.BIN), status.localExecutable);
  }
  const rows = Object.fromEntries(localSetupCommands(env.BIN).map((row) => [row.id, row]));
  ctx.observe("commands", Object.values(rows).map((row) => row.command));

  // A throwaway HOME (and every client home inside it) under the root.
  const home = join(env.ROOT, `guide-home-${tag}`);
  for (const dir of [home, join(home, ".codex"), join(home, ".claude-storage"), join(home, ".hermes")]) mkdirSync(dir, { recursive: true });
  const homes = { HOME: home, CODEX_HOME: join(home, ".codex"), CLAUDE_SECURESTORAGE_CONFIG_DIR: join(home, ".claude-storage"), HERMES_HOME: join(home, ".hermes") };
  const shell = (command, { input = "", timeoutSec = 180 } = {}) =>
    sandboxRun(env, ["/bin/sh", "-c", command], { signal, cwd: home, extraEnv: homes, input, timeoutSec });
  const evidence = (result) => ({ code: result.code, stdout: result.stdout.slice(-600), stderr: result.stderr.slice(-600) });

  // Claude Code
  const claudeAdd = await shell(rows["claude-code"].command);
  const claudeGet = await shell("claude mcp get codemux");
  ctx.check("Claude Code: the command registers codemux and `claude mcp get` reports it connected",
    claudeAdd.code === 0 && claudeGet.code === 0 && /connected/i.test(claudeGet.stdout) && !/failed/i.test(claudeGet.stdout), evidence(claudeGet));
  const claudeConfig = read(join(home, ".claude.json"));
  ctx.check("Claude Code: its user config runs `codemux mcp --client claude-code`",
    claudeConfig.includes(JSON.stringify(env.BIN)) && /"--client",\s*"claude-code"/.test(claudeConfig));

  // Codex
  const codexAdd = await shell(rows.codex.command);
  const codexList = await shell("codex mcp list");
  ctx.check("Codex: the command registers codemux and `codex mcp list` lists it",
    codexAdd.code === 0 && codexList.code === 0 && /\bcodemux\b/.test(codexList.stdout), evidence(codexList));
  const codexConfig = read(join(home, ".codex", "config.toml"));
  ctx.check("Codex: its config runs `codemux mcp --client codex`", /"--client",\s*"codex"/.test(codexConfig), codexConfig.slice(0, 400));

  // Hermes
  if ((env.ORCH_LIVE_PROVIDERS ?? "").split(",").includes("hermes")) {
    // Hermes finds its install generations under its home; the sandbox
    // mounts them under the real one, so link them into the throwaway home.
    symlinkSync(join(process.env.HOME, ".hermes", "installs"), join(home, ".hermes", "installs"));
    // Enter accepts every tool.
    const hermesAdd = await shell(rows.hermes.command, { input: "\n" });
    const hermesTest = await shell("hermes mcp test codemux");
    ctx.check("Hermes: the command registers codemux and `hermes mcp test` lists thread_launch",
      hermesAdd.code === 0 && hermesTest.code === 0 && hermesTest.stdout.includes("thread_launch"), { add: evidence(hermesAdd), test: evidence(hermesTest) });
    const hermesConfig = read(join(home, ".hermes", "config.yaml"));
    ctx.check("Hermes: its config runs `codemux mcp --client hermes --access supervised`",
      hermesConfig.includes("--client") && hermesConfig.includes("hermes") && hermesConfig.includes("supervised"), hermesConfig.slice(0, 400));
  } else {
    ctx.observe("Hermes", "skipped: ORCH_LIVE_PROVIDERS does not include hermes, so its runtime is not mounted");
  }

  // Any other MCP client: the bare command is a working stdio server.
  const other = StdioMcpClient.spawn(env, ["/bin/sh", "-c", rows.other.command], { signal, cwd: home, extraEnv: homes, timeoutSec: 120 });
  try {
    const init = await other.initialize({ name: "guide-setup", version: "1" });
    const tools = (await other.list()).map((tool) => tool.name);
    ctx.check("Other: the bare command serves MCP with the outside tools", init?.serverInfo?.name === "codemux" && tools.includes("thread_launch"),
      { tools: tools.length });
    ctx.check("Other: it exits 0 when the client hangs up", (await other.close()) === 0);
  } finally {
    await other.close();
  }
}
