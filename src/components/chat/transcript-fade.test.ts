import { describe, it, expect, afterEach } from "vitest";

import {
  decideTranscriptFade,
  getRendererMode,
  resetTranscriptFadeCacheForTests,
  setRendererMode,
  subscribeTranscriptFade,
  transcriptFadeEnabled,
} from "./transcript-fade";

// Real-world user-agent strings for each engine Codemux can meet (mirrors the
// probe test's fixtures).
const UA_WEBKITGTK =
  "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";
const UA_CHROME_LINUX =
  "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
const UA_WKWEBVIEW_MACOS =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15";

describe("decideTranscriptFade", () => {
  it("defaults ON for every engine, Linux WebKitGTK included", () => {
    // The mask used to be gated off on Linux WebKitGTK because the webview ran
    // non-composited there. Accelerated compositing is restored, and the mask
    // measures as free, so the design intent applies on all platforms.
    expect(decideTranscriptFade(UA_CHROME_LINUX, null)).toBe(true);
    expect(decideTranscriptFade(UA_WKWEBVIEW_MACOS, null)).toBe(true);
    expect(decideTranscriptFade(UA_WEBKITGTK, null)).toBe(true);
  });

  it("override 'off' disables the mask on any engine", () => {
    expect(decideTranscriptFade(UA_WEBKITGTK, "off")).toBe(false);
    expect(decideTranscriptFade(UA_CHROME_LINUX, "off")).toBe(false);
    expect(decideTranscriptFade(UA_WKWEBVIEW_MACOS, "off")).toBe(false);
  });

  it("override 'on' keeps the mask on", () => {
    expect(decideTranscriptFade(UA_WEBKITGTK, "on")).toBe(true);
    expect(decideTranscriptFade(UA_CHROME_LINUX, "on")).toBe(true);
  });

  it("disables the mask on the compatibility (CPU) renderer", () => {
    // No compositor to hand the mask to — it costs a full-viewport
    // re-rasterization per scroll frame there.
    expect(decideTranscriptFade(UA_WEBKITGTK, null, "compatibility")).toBe(
      false,
    );
    expect(decideTranscriptFade(UA_WEBKITGTK, null, "accelerated")).toBe(true);
  });

  it("override 'on' still wins over the compatibility renderer", () => {
    expect(decideTranscriptFade(UA_WEBKITGTK, "on", "compatibility")).toBe(
      true,
    );
    expect(decideTranscriptFade(UA_WEBKITGTK, "off", "accelerated")).toBe(
      false,
    );
  });
});

describe("renderer mode cache", () => {
  afterEach(() => {
    resetTranscriptFadeCacheForTests();
  });

  it("defaults to accelerated so an unanswered probe keeps the fade", () => {
    expect(getRendererMode()).toBe("accelerated");
    expect(transcriptFadeEnabled()).toBe(true);
  });

  it("invalidates the cached decision and notifies when the mode lands", () => {
    // The transcript can mount before the backend answers, so a late
    // "compatibility" has to reach an already-rendered consumer.
    expect(transcriptFadeEnabled()).toBe(true);
    let notified = 0;
    const unsubscribe = subscribeTranscriptFade(() => {
      notified += 1;
    });

    setRendererMode("compatibility");

    expect(notified).toBe(1);
    expect(transcriptFadeEnabled()).toBe(false);
    unsubscribe();
  });

  it("ignores a repeated mode and stops notifying after unsubscribe", () => {
    let notified = 0;
    const unsubscribe = subscribeTranscriptFade(() => {
      notified += 1;
    });

    setRendererMode("accelerated"); // already the current mode
    expect(notified).toBe(0);

    unsubscribe();
    setRendererMode("compatibility");
    expect(notified).toBe(0);
    expect(transcriptFadeEnabled()).toBe(false);
  });
});
