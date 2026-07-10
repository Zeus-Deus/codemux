import { describe, it, expect } from "vitest";

import { decideTranscriptFade } from "./transcript-fade";

// Real-world user-agent strings for each engine Codemux can meet (mirrors the
// probe test's fixtures).
const UA_WEBKITGTK =
  "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";
const UA_CHROME_LINUX =
  "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
const UA_WKWEBVIEW_MACOS =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15";

describe("decideTranscriptFade", () => {
  it("defaults ON for Chromium / macOS UAs (mask kept where composited)", () => {
    expect(decideTranscriptFade(UA_CHROME_LINUX, null)).toBe(true);
    expect(decideTranscriptFade(UA_WKWEBVIEW_MACOS, null)).toBe(true);
  });

  it("defaults OFF for a Linux WebKitGTK UA (non-composited CPU mask cost)", () => {
    expect(decideTranscriptFade(UA_WEBKITGTK, null)).toBe(false);
  });

  it("override 'on' wins over the WebKitGTK default (forces the mask)", () => {
    expect(decideTranscriptFade(UA_WEBKITGTK, "on")).toBe(true);
  });

  it("override 'off' wins over a Chromium UA (forces no mask)", () => {
    expect(decideTranscriptFade(UA_CHROME_LINUX, "off")).toBe(false);
  });
});
