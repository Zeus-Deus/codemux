import { describe, expect, test } from "bun:test";

import {
  classifyAuthOutput,
  classifyInstallOutcome,
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
