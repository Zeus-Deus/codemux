// The env every SDK query() starts Claude Code with: labelled `codemux`,
// detached from a launching Claude Code session, and overridable from
// Claude's settings `env`.

import { afterEach, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { buildSideChatOptions } from "../src/side-chat.ts";
import {
  CLAUDE_HOST_SESSION_ENV_VARS,
  claudeProcessEnv,
  settingsEntrypoint,
} from "../src/claude-env.ts";

const dirs: string[] = [];
function tempDir(): string {
  const dir = mkdtempSync(join(tmpdir(), "codemux-claude-env-"));
  dirs.push(dir);
  return dir;
}
function configDir(settings?: unknown): string {
  const dir = tempDir();
  if (settings !== undefined) {
    writeFileSync(join(dir, "settings.json"), JSON.stringify(settings));
  }
  return dir;
}
afterEach(() => {
  for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

const installedSdkVersion = (
  JSON.parse(
    readFileSync(
      join(import.meta.dir, "../node_modules/@anthropic-ai/claude-agent-sdk/package.json"),
      "utf8",
    ),
  ) as { version: string }
).version;

test("labels Claude processes codemux, replacing an inherited host label", () => {
  const env = claudeProcessEnv({
    base: { CLAUDE_CONFIG_DIR: configDir(), CLAUDE_CODE_ENTRYPOINT: "claude-desktop", KEEP: "1" },
    platform: "linux",
  });
  expect(env.CLAUDE_CODE_ENTRYPOINT).toBe("codemux");
  expect(env.KEEP).toBe("1");
});

test("drops the launching Claude Code session's variables", () => {
  const host: Record<string, string> = { CLAUDE_CONFIG_DIR: configDir() };
  for (const name of CLAUDE_HOST_SESSION_ENV_VARS) host[name] = "host";
  host.ANTHROPIC_BASE_URL = "http://proxy.invalid";
  host.CLAUDE_CODE_SHELL = "/bin/zsh";
  const env = claudeProcessEnv({ base: host, platform: "linux" });
  for (const name of CLAUDE_HOST_SESSION_ENV_VARS) expect(env[name]).toBeUndefined();
  // User configuration still reaches Claude.
  expect(env.ANTHROPIC_BASE_URL).toBe("http://proxy.invalid");
  expect(env.CLAUDE_CODE_SHELL).toBe("/bin/zsh");
});

test("names the bundled Agent SDK, not one a launching app exported", () => {
  const env = claudeProcessEnv({
    base: { CLAUDE_CONFIG_DIR: configDir(), CLAUDE_AGENT_SDK_VERSION: "0.0.1-host" },
    platform: "linux",
  });
  expect(env.CLAUDE_AGENT_SDK_VERSION).toBe(installedSdkVersion);
});

test("Claude settings env chooses a different label", () => {
  const dir = configDir({ env: { CLAUDE_CODE_ENTRYPOINT: "custom" } });
  expect(
    claudeProcessEnv({ base: { CLAUDE_CONFIG_DIR: dir }, platform: "linux" })
      .CLAUDE_CODE_ENTRYPOINT,
  ).toBe("custom");
});

test("a relative CLAUDE_CONFIG_DIR resolves against the Claude process's folder", () => {
  const cwd = tempDir();
  mkdirSync(join(cwd, "cfg"));
  writeFileSync(
    join(cwd, "cfg", "settings.json"),
    JSON.stringify({ env: { CLAUDE_CODE_ENTRYPOINT: "relative" } }),
  );
  expect(settingsEntrypoint({ CLAUDE_CONFIG_DIR: "cfg" }, "linux", cwd)).toBe("relative");
  expect(
    claudeProcessEnv({ cwd, base: { CLAUDE_CONFIG_DIR: "cfg" }, platform: "linux" })
      .CLAUDE_CODE_ENTRYPOINT,
  ).toBe("relative");
});

test("blank or malformed settings fall back to codemux", () => {
  const blank = configDir({ env: { CLAUDE_CODE_ENTRYPOINT: "  " } });
  expect(settingsEntrypoint({ CLAUDE_CONFIG_DIR: blank }, "linux")).toBeUndefined();
  const broken = configDir();
  writeFileSync(join(broken, "settings.json"), "{not json");
  expect(
    claudeProcessEnv({ base: { CLAUDE_CONFIG_DIR: broken }, platform: "linux" })
      .CLAUDE_CODE_ENTRYPOINT,
  ).toBe("codemux");
});

test("does not mutate the env it is given", () => {
  const base = { CLAUDE_CONFIG_DIR: configDir(), CLAUDECODE: "1" };
  claudeProcessEnv({ base, platform: "linux" });
  expect(base.CLAUDECODE).toBe("1");
});

test("side chats run with the labelled env", () => {
  const options = buildSideChatOptions({
    sideId: "side-env",
    cwd: "/tmp/repo",
    resume: "parent",
    pathToClaudeCodeExecutable: "/usr/bin/claude",
    text: "why?",
  });
  expect(options.env?.CLAUDE_CODE_ENTRYPOINT).toBeTruthy();
  expect(options.env?.CLAUDE_CODE_ENTRYPOINT?.startsWith("sdk-")).toBe(false);
});
