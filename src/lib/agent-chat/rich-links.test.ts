import { describe, expect, it } from "vitest";

import { externalWebLinkHost, isPublicWebHost } from "./rich-links";

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

describe("isPublicWebHost", () => {
  const cases: Array<[string, boolean]> = [
    ["github.com", true],
    ["docs.codemux.org", true],
    ["sub.domain.example.co.uk", true],
    ["EXAMPLE.COM", true],
    ["example.com.", true],
    ["8.8.8.8", true],
    ["localhost", false],
    ["intranet", false],
    ["build.local", false],
    ["app.localhost", false],
    ["wiki.internal", false],
    ["nas.lan", false],
    ["printer.home", false],
    ["jira.corp", false],
    ["192.168.1.1", false],
    ["10.0.0.5", false],
    ["172.16.4.9", false],
    ["172.32.4.9", true],
    ["127.0.0.1", false],
    ["169.254.10.1", false],
    ["0.0.0.0", false],
    ["[::1]", false],
    ["::1", false],
    ["[fc00::1]", false],
    ["[fd12:3456::1]", false],
    ["[fe80::1]", false],
    ["[2606:4700::1111]", true],
    ["", false],
    ["   ", false],
  ];

  it.each(cases)("%s -> public: %s", (host, expected) => {
    expect(isPublicWebHost(host)).toBe(expected);
  });

  it("rejects non-string hosts", () => {
    expect(isPublicWebHost(undefined)).toBe(false);
    expect(isPublicWebHost(null)).toBe(false);
    expect(isPublicWebHost(123)).toBe(false);
  });
});
