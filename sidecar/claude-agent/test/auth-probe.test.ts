import { describe, expect, test } from "bun:test";

import {
  classifyAuthOutput,
  classifyAuthStatus,
  classifyInstallOutcome,
  customEndpointHost,
  parseCliVersion,
  probeInstalled,
} from "../src/auth-probe";

describe("classifyAuthOutput", () => {
  test("JSON loggedIn:true is authenticated", () => {
    const out = JSON.stringify({
      loggedIn: true,
      authMethod: "claude.ai",
      subscriptionType: "max",
    });
    expect(classifyAuthOutput(out).status).toBe("authenticated");
  });

  test("JSON loggedIn:false is unauthenticated", () => {
    const out = `${JSON.stringify({ loggedIn: false })}\n`;
    expect(classifyAuthOutput(out).status).toBe("unauthenticated");
  });

  test("JSON with stderr noise around it still parses", () => {
    const out = `warning: something\n{"loggedIn":true}\n`;
    expect(classifyAuthOutput(out).status).toBe("authenticated");
  });

  test("legacy text output still classifies", () => {
    expect(classifyAuthOutput("Logged in as x@y.z").status).toBe(
      "authenticated",
    );
    expect(classifyAuthOutput("You are not logged in").status).toBe(
      "unauthenticated",
    );
  });

  test("unparseable output is unknown", () => {
    expect(classifyAuthOutput("{ garbage").status).toBe("unknown");
    expect(classifyAuthOutput("").status).toBe("unknown");
  });

  test("the CLI's real pretty-printed payload is authenticated", () => {
    // Verbatim shape of `claude auth status` on a logged-in machine.
    const out = [
      "{",
      '  "loggedIn": true,',
      '  "authMethod": "claude.ai",',
      '  "apiProvider": "firstParty",',
      '  "email": "user@example.com",',
      '  "subscriptionType": "max"',
      "}",
      "",
    ].join("\n");
    expect(classifyAuthOutput(out).status).toBe("authenticated");
  });

  test("brace-bearing noise AFTER the payload does not defeat parsing", () => {
    // The classified text is stdout+stderr concatenated, so anything the
    // CLI adds on stderr lands after the JSON. Slicing first-`{` to
    // last-`}` would swallow it and fall back to `unknown`, which is the
    // stale "could not verify" banner all over again.
    const out = `{"loggedIn":true}\n(node:1) Warning: legacy config {a}\n`;
    expect(classifyAuthOutput(out).status).toBe("authenticated");
  });

  test("a second JSON object does not defeat parsing", () => {
    const out = `{"loggedIn":false}\n{"update":"available"}\n`;
    expect(classifyAuthOutput(out).status).toBe("unauthenticated");
  });

  test("braces inside string values do not unbalance the scan", () => {
    const out = JSON.stringify({ orgName: "Acme {Labs}", loggedIn: true });
    expect(classifyAuthOutput(out).status).toBe("authenticated");
  });

  test("`unauthenticated` is not read as `authenticated`", () => {
    // "authenticated" is a substring of "unauthenticated"; matching it
    // positively would hide the banner from a logged-OUT user.
    expect(classifyAuthOutput("Status: unauthenticated").status).toBe(
      "unauthenticated",
    );
  });

  test("a JSON object without `loggedIn` falls through to the text rules", () => {
    expect(classifyAuthOutput(`{"other":1}\nnot logged in`).status).toBe(
      "unauthenticated",
    );
    expect(classifyAuthOutput(`{"other":1}`).status).toBe("unknown");
  });

  test("non-boolean `loggedIn` does not fabricate an answer", () => {
    // A string/null value is an unrecognized schema, not a "yes".
    expect(classifyAuthOutput(`{"loggedIn":"true"}`).status).toBe("unknown");
    expect(classifyAuthOutput(`{"loggedIn":null}`).status).toBe("unknown");
  });
});

/** `claude auth status` JSON as Claude Code 2.1.317 prints it. */
function statusJson(fields: Record<string, unknown>): string {
  return `${JSON.stringify(
    {
      loggedIn: false,
      authMethod: "none",
      apiProvider: "firstParty",
      analyticsDisabled: false,
      projectsDirectory: "/tmp/cfg/projects",
      configDirectory: "/tmp/cfg",
      ...fields,
    },
    null,
    2,
  )}\n`;
}

describe("classifyAuthStatus", () => {
  test("settings.json env proxy is a custom endpoint, not a Claude login", () => {
    // ANTHROPIC_BASE_URL + ANTHROPIC_AUTH_TOKEN: the JSON calls the proxy
    // token an "oauth_token", so only the base URL tells it apart.
    const result = classifyAuthStatus(
      statusJson({ loggedIn: true, authMethod: "oauth_token" }),
      "Auth token: ANTHROPIC_AUTH_TOKEN\nAnthropic base URL: https://***@gateway.example.com:8443/v1?x=1\n",
    );
    expect(result).toEqual({
      status: "authenticated",
      authLabel: "Custom endpoint · gateway.example.com:8443",
    });
  });

  test("a base URL with no token is ready even though the CLI says logged out", () => {
    const result = classifyAuthStatus(
      statusJson({ loggedIn: false }),
      "Anthropic base URL: http://127.0.0.1:8317\nNot logged in. Run claude auth login to authenticate.\n",
    );
    expect(result).toEqual({
      status: "authenticated",
      authLabel: "Custom endpoint · 127.0.0.1:8317",
    });
  });

  test("an apiKeyHelper is labelled as such", () => {
    const result = classifyAuthStatus(
      statusJson({
        loggedIn: true,
        authMethod: "api_key_helper",
        apiKeySource: "apiKeyHelper",
      }),
      "Auth token: apiKeyHelper\n",
    );
    expect(result).toEqual({ status: "authenticated", authLabel: "API key helper" });
  });

  test("an apiKeyHelper behind a proxy is labelled by the proxy", () => {
    const result = classifyAuthStatus(
      statusJson({ loggedIn: true, authMethod: "api_key_helper" }),
      "Auth token: apiKeyHelper\nAnthropic base URL: https://llm.internal/anthropic\n",
    );
    expect(result.authLabel).toBe("Custom endpoint · llm.internal");
  });

  test("ANTHROPIC_API_KEY is an Anthropic API key", () => {
    const result = classifyAuthStatus(
      statusJson({
        loggedIn: true,
        authMethod: "api_key",
        apiKeySource: "ANTHROPIC_API_KEY",
      }),
      "API key: ANTHROPIC_API_KEY\n",
    );
    expect(result).toEqual({ status: "authenticated", authLabel: "Anthropic API key" });
  });

  test("Bedrock is ready and named", () => {
    const result = classifyAuthStatus(
      statusJson({ loggedIn: true, authMethod: "third_party", apiProvider: "bedrock" }),
      "API provider: Amazon Bedrock\nAWS region: us-east-1\n",
    );
    expect(result).toEqual({ status: "authenticated", authLabel: "Amazon Bedrock" });
  });

  test("a cloud provider counts as ready even when loggedIn is false", () => {
    const vertex = classifyAuthStatus(
      statusJson({ loggedIn: false, apiProvider: "vertex" }),
      "",
    );
    expect(vertex).toEqual({ status: "authenticated", authLabel: "Google Vertex AI" });
    const foundry = classifyAuthStatus(
      statusJson({ loggedIn: true, apiProvider: "foundry" }),
      "",
    );
    expect(foundry.authLabel).toBe("Microsoft Foundry");
  });

  test("an unknown cloud provider borrows the CLI's own name", () => {
    const result = classifyAuthStatus(
      statusJson({ loggedIn: true, apiProvider: "someCloud" }),
      "API provider: Some Cloud\n",
    );
    expect(result).toEqual({ status: "authenticated", authLabel: "Some Cloud" });
  });

  test("the Cloud gateway still needs its sign-in", () => {
    expect(
      classifyAuthStatus(statusJson({ loggedIn: false, apiProvider: "gateway" }), "")
        .status,
    ).toBe("unauthenticated");
  });

  test("a real claude.ai login shows the plan", () => {
    const result = classifyAuthStatus(
      statusJson({
        loggedIn: true,
        authMethod: "claude.ai",
        email: "user@example.com",
        subscriptionType: "max",
      }),
      "Login method: Claude Max account\nEmail: user@example.com\n",
    );
    expect(result).toEqual({ status: "authenticated", authLabel: "Claude Max" });
  });

  test("plan falls back to the text login method, then to a plain account", () => {
    expect(
      classifyAuthStatus(
        statusJson({ loggedIn: true, authMethod: "claude.ai", subscriptionType: null }),
        "Login method: Claude Team account\n",
      ).authLabel,
    ).toBe("Claude Team");
    expect(
      classifyAuthStatus(
        statusJson({ loggedIn: true, authMethod: "claude.ai" }),
        "",
      ).authLabel,
    ).toBe("Claude account");
    expect(
      classifyAuthStatus(
        statusJson({
          loggedIn: true,
          authMethod: "claude.ai",
          subscriptionType: "claude_enterprise_subscription",
        }),
        "",
      ).authLabel,
    ).toBe("Claude Enterprise");
  });

  test("a CLAUDE_CODE_OAUTH_TOKEN without a base URL is a Claude account", () => {
    expect(
      classifyAuthStatus(statusJson({ loggedIn: true, authMethod: "oauth_token" }), "")
        .authLabel,
    ).toBe("Claude account");
  });

  test("not logged in stays signed out", () => {
    const result = classifyAuthStatus(
      statusJson({ loggedIn: false }),
      "Not logged in. Run claude auth login to authenticate.\n",
    );
    expect(result.status).toBe("unauthenticated");
    expect(result.authLabel).toBeUndefined();
  });

  test("a first-party base URL is not a custom endpoint", () => {
    const firstParty = ["https://api", "anthropic", "com"].join(".");
    expect(
      classifyAuthStatus(
        statusJson({ loggedIn: false }),
        `Anthropic base URL: ${firstParty}\n`,
      ).status,
    ).toBe("unauthenticated");
  });

  test("a legacy text-only CLI keeps the old heuristics", () => {
    // `--text` is rejected as an unknown option; plain output is text.
    expect(
      classifyAuthStatus("Logged in as x@y.z\n", "error: unknown option '--text'\n"),
    ).toEqual({ status: "authenticated" });
    expect(classifyAuthStatus("You are not logged in\n", "").status).toBe(
      "unauthenticated",
    );
    expect(classifyAuthStatus("", "").status).toBe("unknown");
    expect(
      classifyAuthStatus("Login method: Claude Pro account\nLogged in\n", "")
        .authLabel,
    ).toBe("Claude Pro");
  });
});

describe("customEndpointHost", () => {
  test("keeps only host and port", () => {
    expect(customEndpointHost("https://u:p@proxy.example:4000/v1?key=1#x")).toBe(
      "proxy.example:4000",
    );
    expect(customEndpointHost("https://proxy.example:443/")).toBe("proxy.example");
    expect(customEndpointHost("http://[::1]:8317")).toBe("[::1]:8317");
  });

  test("rejects unset, malformed, non-http and first-party URLs", () => {
    expect(customEndpointHost(undefined)).toBeNull();
    expect(customEndpointHost("not a url")).toBeNull();
    expect(customEndpointHost("unix:///tmp/sock")).toBeNull();
    expect(customEndpointHost(`https://${["api", "anthropic", "com"].join(".")}`)).toBeNull();
    expect(customEndpointHost(`https://${["API", "ANTHROPIC", "COM."].join(".")}`)).toBeNull();
  });
});

describe("parseCliVersion", () => {
  test("supports version-first and name-first CLI output", () => {
    expect(parseCliVersion("2.1.283 (Claude Code)\n")).toBe("2.1.283");
    expect(parseCliVersion("Claude Code 2.1.114")).toBe("2.1.114");
    expect(parseCliVersion("claude 2.2.0-beta.1")).toBe("2.2.0-beta.1");
    expect(parseCliVersion("unexpected output")).toBe("unknown");
  });
});

describe("classifyInstallOutcome", () => {
  test("a missing binary is not installed", () => {
    const error = Object.assign(new Error("spawn claude ENOENT"), {
      code: "ENOENT",
    });
    expect(
      classifyInstallOutcome({ stdout: "", exitCode: null, error, timedOut: false }),
    ).toEqual({ installed: false });
  });

  test("a timeout is installed but unresponsive, not missing", () => {
    expect(
      classifyInstallOutcome({ stdout: "", exitCode: null, timedOut: true }),
    ).toEqual({ installed: true, unresponsive: true });
  });

  test("a non-zero exit is installed but unresponsive", () => {
    expect(
      classifyInstallOutcome({ stdout: "", exitCode: 1, timedOut: false }),
    ).toEqual({ installed: true, unresponsive: true });
  });

  test("a permission error is installed but unresponsive", () => {
    const error = Object.assign(new Error("spawn claude EACCES"), {
      code: "EACCES",
    });
    expect(
      classifyInstallOutcome({ stdout: "", exitCode: null, error, timedOut: false }),
    ).toEqual({ installed: true, unresponsive: true });
  });

  test("a clean run reports the version", () => {
    expect(
      classifyInstallOutcome({
        stdout: "2.1.114 (Claude Code)\n",
        exitCode: 0,
        timedOut: false,
      }),
    ).toEqual({ installed: true, version: "2.1.114" });
  });
});

describe("probeInstalled", () => {
  test("a binary that does not exist is not installed", async () => {
    expect(await probeInstalled("/nonexistent/codemux-claude-probe")).toEqual({
      installed: false,
    });
  });

  test("a binary that exits non-zero is unresponsive", async () => {
    expect(await probeInstalled("false")).toEqual({
      installed: true,
      unresponsive: true,
    });
  });
});
