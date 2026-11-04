// Tests for `src/user-routing.ts`: which settings.json entries reach an
// isolated helper query, and how they combine with the inherited env.

import { afterEach, describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";

import { routedOptions, settingsPath, userRouting } from "../src/user-routing.ts";

const homes: string[] = [];

function homeWith(settings: unknown): string {
  const home = mkdtempSync(join(tmpdir(), "routing-"));
  homes.push(home);
  writeFileSync(
    join(home, "settings.json"),
    typeof settings === "string" ? settings : JSON.stringify(settings),
  );
  return home;
}

afterEach(() => {
  for (const home of homes.splice(0)) rmSync(home, { recursive: true, force: true });
});

const PROXY = {
  env: {
    ANTHROPIC_BASE_URL: "http://127.0.0.1:8317",
    ANTHROPIC_AUTH_TOKEN: "dummy-token",
    API_TIMEOUT_MS: 60000,
    CLAUDE_CODE_USE_BEDROCK: true,
    // Not routing: must stay out of isolated helpers.
    CLAUDE_CODE_DISABLE_CLAUDE_MDS: "0",
    DISABLE_TELEMETRY: "1",
  },
  apiKeyHelper: "/usr/local/bin/print-key",
  outputStyle: "Pirate",
  permissions: { allow: ["Bash(*)"] },
  enabledPlugins: { "dangerous@market": true },
  hooks: { PreToolUse: [] },
  model: "opus",
};

describe("userRouting", () => {
  test("forwards routing env and apiKeyHelper, nothing else", () => {
    const home = homeWith(PROXY);
    const routing = userRouting({ CLAUDE_CONFIG_DIR: home, PATH: "/bin" });
    expect(routing.env).toEqual({
      CLAUDE_CONFIG_DIR: home,
      PATH: "/bin",
      ANTHROPIC_BASE_URL: "http://127.0.0.1:8317",
      ANTHROPIC_AUTH_TOKEN: "dummy-token",
      API_TIMEOUT_MS: "60000",
      CLAUDE_CODE_USE_BEDROCK: "true",
    });
    // Only the command name goes to `settings` (argv); no secrets, and no
    // output style, permissions, plugins, hooks or model.
    expect(routing.settings).toEqual({ apiKeyHelper: "/usr/local/bin/print-key" });
  });

  test("settings.json routing overrides the inherited env; other inherited values stay", () => {
    const home = homeWith({ env: { ANTHROPIC_BASE_URL: "http://settings:1" } });
    const routing = userRouting({
      CLAUDE_CONFIG_DIR: home,
      ANTHROPIC_BASE_URL: "http://shell:2",
      ANTHROPIC_AUTH_TOKEN: "from-shell",
    });
    expect(routing.env.ANTHROPIC_BASE_URL).toBe("http://settings:1");
    expect(routing.env.ANTHROPIC_AUTH_TOKEN).toBe("from-shell");
  });

  test("reads the bound account home, else ~/.claude", () => {
    const work = join(tmpdir(), "accounts", "work");
    expect(settingsPath({ CLAUDE_CONFIG_DIR: work })).toBe(join(work, "settings.json"));
    expect(settingsPath({ CLAUDE_CONFIG_DIR: "" })).toBe(join(homedir(), ".claude", "settings.json"));
    expect(settingsPath({})).toBe(join(homedir(), ".claude", "settings.json"));
  });

  test("a missing, malformed or odd settings file adds nothing", () => {
    const base = { PATH: "/bin" };
    for (const env of [
      { ...base, CLAUDE_CONFIG_DIR: join(tmpdir(), "no-such-claude-home") },
      { ...base, CLAUDE_CONFIG_DIR: homeWith("{ not json") },
      { ...base, CLAUDE_CONFIG_DIR: homeWith([1, 2]) },
      { ...base, CLAUDE_CONFIG_DIR: homeWith({ env: ["x"], apiKeyHelper: " " }) },
      { ...base, CLAUDE_CONFIG_DIR: homeWith({ env: { ANTHROPIC_BASE_URL: { url: "x" } } }) },
    ]) {
      const routing = userRouting(env);
      expect(routing.env).toEqual(env);
      expect(routing.settings).toEqual({});
    }
  });

  test("routedOptions stays isolated and only sets settings for a helper", () => {
    const proxy = routedOptions({ CLAUDE_CONFIG_DIR: homeWith({ env: { ANTHROPIC_BASE_URL: "http://p:1" } }) });
    expect(proxy.settingSources).toEqual([]);
    expect(proxy.env?.ANTHROPIC_BASE_URL).toBe("http://p:1");
    expect(proxy.settings).toBeUndefined();
    const helper = routedOptions({ CLAUDE_CONFIG_DIR: homeWith({ apiKeyHelper: "key-cmd" }) });
    expect(helper.settings).toEqual({ apiKeyHelper: "key-cmd" });
    // Bedrock/Vertex credential commands travel without any apiKeyHelper.
    const cloud = routedOptions({
      CLAUDE_CONFIG_DIR: homeWith({
        env: { CLAUDE_CODE_USE_BEDROCK: "1" },
        awsCredentialExport: "aws-export-cmd",
        awsAuthRefresh: "aws-sso-cmd",
        gcpAuthRefresh: "gcloud-cmd",
        outputStyle: "Explanatory",
      }),
    });
    expect(cloud.env?.CLAUDE_CODE_USE_BEDROCK).toBe("1");
    expect(cloud.settings).toEqual({
      awsCredentialExport: "aws-export-cmd",
      awsAuthRefresh: "aws-sso-cmd",
      gcpAuthRefresh: "gcloud-cmd",
    });
  });
});
