import { describe, expect, it } from "vitest";

import { externalWebLinkHost, trustedFaviconHost } from "./rich-links";

describe("externalWebLinkHost", () => {
  it("returns the hostname for absolute http(s) links", () => {
    expect(externalWebLinkHost("https://github.com/example/repo/pull/235")).toBe(
      "github.com",
    );
    expect(externalWebLinkHost("http://docs.codemux.org")).toBe("docs.codemux.org");
  });

  it("never decorates non-web schemes", () => {
    for (const href of [
      "javascript:alert(1)",
      "JavaScript:alert(1)",
      "data:text/html;base64,PHNjcmlwdD4=",
      "vbscript:msgbox(1)",
      "file:///etc/passwd",
      "mailto:hello@codemux.org",
      "#code-blocks",
      "./relative/path",
      "",
      null,
      undefined,
      42,
    ]) {
      expect(externalWebLinkHost(href)).toBeNull();
    }
  });
});

describe("trustedFaviconHost", () => {
  it.each([
    ["github.com", "github.com"],
    ["GITHUB.COM", "github.com"],
    ["github.com.", "github.com"],
    ["docs.codemux.org", "docs.codemux.org"],
    ["gitlab.com", "gitlab.com"],
    ["developer.mozilla.org", "developer.mozilla.org"],
    ["platform.openai.com", "platform.openai.com"],
    ["git.corp.example.com", null],
    ["private.github.com", null],
    ["github.com.attacker.test", null],
    ["github.com ", null],
    ["example.com", null],
    ["8.8.8.8", null],
    ["[2001:4860:4860::8888]", null],
    ["localhost", null],
    ["service.home.arpa", null],
    ["", null],
  ])("%s -> %s", (host, expected) => {
    expect(trustedFaviconHost(host)).toBe(expected);
  });

  it("rejects non-string hosts", () => {
    expect(trustedFaviconHost(undefined)).toBeNull();
    expect(trustedFaviconHost(null)).toBeNull();
    expect(trustedFaviconHost(123)).toBeNull();
  });
});
