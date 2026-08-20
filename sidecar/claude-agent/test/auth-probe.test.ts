import { describe, expect, test } from "bun:test";

import { classifyAuthOutput } from "../src/auth-probe";

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
