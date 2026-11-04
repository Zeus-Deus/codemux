// Provider routing for isolated helper queries (model list, usage, side
// chats, managed workflows).
//
// Those queries run with `settingSources: []` so nothing from the user's
// setup (CLAUDE.md, output style, plugins, hooks, permission rules) can
// reach them. Proxies and gateways, however, are usually configured in
// the Claude home's `settings.json` (`env.ANTHROPIC_BASE_URL`, a token,
// `apiKeyHelper`), which isolation also skips, so the helper would fall
// back to a direct, unauthenticated connection. This module copies only
// the routing entries of that file:
//
// - routing `env` values go into the child environment, never argv (the
//   SDK puts `settings` on the command line, and these can be secrets);
// - `apiKeyHelper` is a command, not a secret, so it rides in `settings`.
//
// The values are passed through unread; the CLI authenticates with them
// exactly as it would when loading the file itself. That is why this file
// is the one exception to the ToS check's credential-name rule.

import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

import type { Options } from "@anthropic-ai/claude-agent-sdk";

type Env = Record<string, string | undefined>;

/** settings.json `env` keys that choose where and how the CLI reaches a
 *  model. Everything else in that block is user preference and stays
 *  out of isolated helpers. Names checked against Claude Code 2.1.317.
 *  Keep in step with `CLAUDE_ROUTING_ENV_KEYS` in
 *  src-tauri/src/agent_provider/claude/auth.rs. */
export const ROUTING_ENV_KEYS: readonly string[] = [
  // Direct API / proxy
  "ANTHROPIC_BASE_URL",
  "ANTHROPIC_AUTH_TOKEN",
  "ANTHROPIC_API_KEY",
  "ANTHROPIC_CUSTOM_HEADERS",
  "CLAUDE_CODE_OAUTH_TOKEN",
  "API_TIMEOUT_MS",
  // Network
  "HTTPS_PROXY",
  "HTTP_PROXY",
  "NO_PROXY",
  "NODE_EXTRA_CA_CERTS",
  "CLAUDE_CODE_CLIENT_CERT",
  "CLAUDE_CODE_CLIENT_KEY",
  "CLAUDE_CODE_CLIENT_KEY_PASSPHRASE",
  // Amazon Bedrock
  "CLAUDE_CODE_USE_BEDROCK",
  "ANTHROPIC_BEDROCK_BASE_URL",
  "AWS_BEARER_TOKEN_BEDROCK",
  "CLAUDE_CODE_SKIP_BEDROCK_AUTH",
  "CLAUDE_CODE_USE_MANTLE",
  "ANTHROPIC_BEDROCK_MANTLE_BASE_URL",
  "CLAUDE_CODE_SKIP_MANTLE_AUTH",
  "AWS_REGION",
  "AWS_DEFAULT_REGION",
  "AWS_PROFILE",
  "AWS_ACCESS_KEY_ID",
  "AWS_SECRET_ACCESS_KEY",
  "AWS_SESSION_TOKEN",
  "AWS_CONFIG_FILE",
  "AWS_SHARED_CREDENTIALS_FILE",
  // Google Vertex AI
  "CLAUDE_CODE_USE_VERTEX",
  "ANTHROPIC_VERTEX_BASE_URL",
  "ANTHROPIC_VERTEX_PROJECT_ID",
  "CLAUDE_CODE_SKIP_VERTEX_AUTH",
  "CLOUD_ML_REGION",
  "GOOGLE_CLOUD_PROJECT",
  "GOOGLE_APPLICATION_CREDENTIALS",
  // Microsoft Foundry
  "CLAUDE_CODE_USE_FOUNDRY",
  "ANTHROPIC_FOUNDRY_RESOURCE",
  "ANTHROPIC_FOUNDRY_BASE_URL",
  "ANTHROPIC_FOUNDRY_API_KEY",
  "ANTHROPIC_FOUNDRY_AUTH_TOKEN",
  "CLAUDE_CODE_SKIP_FOUNDRY_AUTH",
];

export interface UserRouting {
  /** `baseEnv` with the settings.json routing values laid over it.
   *  Settings values win, matching the CLI, which applies its `env`
   *  block over the inherited environment. */
  env: Env;
  /** Flag-settings entries to merge into the SDK `settings` option. */
  settings: { apiKeyHelper?: string };
}

/** The settings.json the CLI would read for this environment: the
 *  account home in `CLAUDE_CONFIG_DIR` when one is bound, else
 *  `~/.claude`. */
export function settingsPath(env: Env): string {
  const home = env.CLAUDE_CONFIG_DIR || join(homedir(), ".claude");
  return join(home, "settings.json");
}

/** Routing for a helper whose child will run with `baseEnv`. A missing
 *  or unreadable settings file just means no extra routing. */
export function userRouting(baseEnv: Env, read = readSettings): UserRouting {
  const settings = read(settingsPath(baseEnv));
  const env: Env = { ...baseEnv };
  const block = settings?.["env"];
  if (isRecord(block)) {
    for (const key of ROUTING_ENV_KEYS) {
      const value = block[key];
      // The CLI stringifies scalar values in this block.
      if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
        env[key] = String(value);
      }
    }
  }
  const helper = settings?.["apiKeyHelper"];
  return {
    env,
    settings: typeof helper === "string" && helper.trim() ? { apiKeyHelper: helper } : {},
  };
}

/** Options for a minimal isolated query (model list, usage): no setting
 *  sources, the routed environment, and `apiKeyHelper` when set. */
export function routedOptions(
  baseEnv: Env = process.env as Env,
): Pick<Options, "env" | "settingSources" | "settings"> {
  const routing = userRouting(baseEnv);
  return {
    env: routing.env,
    settingSources: [],
    ...(routing.settings.apiKeyHelper ? { settings: routing.settings } : {}),
  };
}

function readSettings(path: string): Record<string, unknown> | null {
  try {
    const parsed: unknown = JSON.parse(readFileSync(path, "utf8"));
    return isRecord(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
