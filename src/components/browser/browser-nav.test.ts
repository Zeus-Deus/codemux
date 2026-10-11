import { describe, expect, it } from "vitest";

import { BROWSER_SEARCH_URL, browserChromeShortcut, normalizeBrowserUrl } from "./browser-nav";

describe("normalizeBrowserUrl", () => {
  it.each([
    ["localhost", "http://localhost"],
    ["localhost:5173", "http://localhost:5173"],
    ["localhost:3000/admin?x=1", "http://localhost:3000/admin?x=1"],
    ["127.0.0.1:8080", "http://127.0.0.1:8080"],
    ["0.0.0.0:4000", "http://0.0.0.0:4000"],
    ["[::1]:3000", "http://[::1]:3000"],
    ["192.168.1.20", "http://192.168.1.20"],
    ["myapp.local", "http://myapp.local"],
    ["api.test/health", "http://api.test/health"],
    ["devbox:8080", "http://devbox:8080"],
  ])("opens local dev target %s over http", (input, expected) => {
    expect(normalizeBrowserUrl(input)).toBe(expected);
  });

  it("opens ordinary dotted hosts over https", () => {
    expect(normalizeBrowserUrl("example.com")).toBe("https://example.com");
    expect(normalizeBrowserUrl("  docs.rs/serde ")).toBe("https://docs.rs/serde");
  });

  it("keeps explicit schemes untouched", () => {
    expect(normalizeBrowserUrl("https://localhost:5173")).toBe("https://localhost:5173");
    expect(normalizeBrowserUrl("about:blank")).toBe("about:blank");
    expect(normalizeBrowserUrl("data:text/html,hi")).toBe("data:text/html,hi");
  });

  it("treats words and phrases as a search", () => {
    expect(normalizeBrowserUrl("react hooks")).toBe(`${BROWSER_SEARCH_URL}react%20hooks`);
    expect(normalizeBrowserUrl("tailwind")).toBe(`${BROWSER_SEARCH_URL}tailwind`);
  });
});

describe("browserChromeShortcut", () => {
  type Mods = Partial<Record<"ctrlKey" | "metaKey" | "altKey" | "shiftKey", boolean>>;
  const keys = (key: string, mods: Mods = {}) => ({
    key,
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    shiftKey: false,
    ...mods,
  });

  it("maps browser history, reload and address-bar keys", () => {
    expect(browserChromeShortcut(keys("ArrowLeft", { altKey: true }))).toBe("back");
    expect(browserChromeShortcut(keys("ArrowRight", { altKey: true }))).toBe("forward");
    expect(browserChromeShortcut(keys("r", { ctrlKey: true }))).toBe("reload");
    expect(browserChromeShortcut(keys("F5"))).toBe("reload");
    expect(browserChromeShortcut(keys("l", { ctrlKey: true }))).toBe("address");
    expect(browserChromeShortcut(keys("L", { metaKey: true }))).toBe("address");
  });

  it("leaves everything else to the page", () => {
    expect(browserChromeShortcut(keys("ArrowLeft"))).toBeNull();
    expect(browserChromeShortcut(keys("ArrowLeft", { altKey: true, shiftKey: true }))).toBeNull();
    expect(browserChromeShortcut(keys("R", { ctrlKey: true, shiftKey: true }))).toBeNull();
    expect(browserChromeShortcut(keys("l"))).toBeNull();
    expect(browserChromeShortcut(keys("c", { ctrlKey: true }))).toBeNull();
  });
});
