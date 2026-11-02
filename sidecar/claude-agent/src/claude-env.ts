// The environment every Claude Code process the sidecar starts runs with.
//
// Claude Code labels its API requests with `CLAUDE_CODE_ENTRYPOINT`. The
// Agent SDK fills in `sdk-ts` when the variable is unset. t3code measured
// `sdk-*` requests draining a subscription's 5-hour window about 1.8x faster
// than other labels (pingdotgg/t3code#25597). Codemux is the entrypoint, so
// it says so. When
// Codemux was started from inside a Claude Code session, that session's
// label (`claude-desktop`, `cli`, ...) and its session variables are
// inherited too; both are dropped so a chat never joins the launching
// session.

import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";

// The SDK does not export its package.json; the bundler inlines this.
import sdkPackage from "../node_modules/@anthropic-ai/claude-agent-sdk/package.json" with { type: "json" };

export const CODEMUX_CLAUDE_ENTRYPOINT = "codemux";

/** Exported by the Claude Code session that launched Codemux. A child
 *  started with them acts as part of that session. */
export const CLAUDE_HOST_SESSION_ENV_VARS = [
  "CLAUDECODE",
  "CLAUDE_CODE_SESSION_ID",
  "CLAUDE_CODE_HOST_SESSION_ID",
  "CLAUDE_CODE_CHILD_SESSION",
  "CLAUDE_CODE_SESSION_ATTENDED",
  "CLAUDE_CODE_SSE_PORT",
  "CLAUDE_CODE_MESSAGING_SOCKET",
  "CLAUDE_CODE_MESSAGING_TOKEN",
  "CLAUDE_CODE_EXECPATH",
  "CLAUDE_CODE_DESKTOP_APP_VERSION",
  "CLAUDE_EFFORT",
  "CLAUDE_PID",
] as const;

type Env = Record<string, string | undefined>;

/** Claude Code's managed-settings file (its hardcoded per-OS location). */
function managedSettingsPath(platform: NodeJS.Platform): string {
  if (platform === "darwin") return "/Library/Application Support/ClaudeCode/managed-settings.json";
  if (platform === "win32") return "C:\\Program Files\\ClaudeCode\\managed-settings.json";
  return "/etc/claude-code/managed-settings.json";
}

/** User settings first, managed settings last: later files win. A relative
 *  `CLAUDE_CONFIG_DIR` is resolved the way the Claude child resolves it,
 *  against the folder it runs in. */
function settingsPaths(env: Env, platform: NodeJS.Platform, cwd: string | undefined): string[] {
  const configured = env.CLAUDE_CONFIG_DIR?.trim();
  const configDir = configured
    ? isAbsolute(configured)
      ? configured
      : resolve(cwd ?? process.cwd(), configured)
    : join(homedir(), ".claude");
  return [join(configDir, "settings.json"), managedSettingsPath(platform)];
}

/** `env.CLAUDE_CODE_ENTRYPOINT` from Claude's settings files. Claude Code
 *  reads the entrypoint only from its process env and ignores this
 *  setting, so the sidecar applies it: it is the user's way to choose a
 *  different label. */
export function settingsEntrypoint(
  env: Env,
  platform: NodeJS.Platform = process.platform,
  cwd?: string,
): string | undefined {
  let entrypoint: string | undefined;
  for (const path of settingsPaths(env, platform, cwd)) {
    let parsed: unknown;
    try {
      parsed = JSON.parse(readFileSync(path, "utf8"));
    } catch {
      continue;
    }
    const value = (parsed as { env?: { CLAUDE_CODE_ENTRYPOINT?: unknown } } | null)?.env
      ?.CLAUDE_CODE_ENTRYPOINT;
    if (typeof value === "string" && value.trim()) entrypoint = value.trim();
  }
  return entrypoint;
}

export interface ClaudeProcessEnvOptions {
  /** Folder the Claude process runs in. */
  cwd?: string;
  base?: Env;
  platform?: NodeJS.Platform;
}

/** A copy of the sidecar's env to hand to the SDK's `query()` as
 *  `options.env`. */
export function claudeProcessEnv({
  cwd,
  base = process.env,
  platform = process.platform,
}: ClaudeProcessEnvOptions = {}): Env {
  const env: Env = { ...base };
  for (const name of CLAUDE_HOST_SESSION_ENV_VARS) delete env[name];
  env.CLAUDE_CODE_ENTRYPOINT = settingsEntrypoint(env, platform, cwd) ?? CODEMUX_CLAUDE_ENTRYPOINT;
  // The SDK sets this on `process.env` inside `query()`, after this copy is
  // made; Claude Code reports it next to the label. Name the SDK actually
  // in use rather than a launching app's.
  env.CLAUDE_AGENT_SDK_VERSION = sdkPackage.version;
  return env;
}
