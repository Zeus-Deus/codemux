import { describe, it, expect, afterEach } from "vitest";

import {
  decideTranscriptFade,
  getRendererMode,
  resetTranscriptFadeCacheForTests,
  setRendererMode,
  subscribeTranscriptFade,
  transcriptFadeEnabled,
} from "./transcript-fade";

describe("decideTranscriptFade", () => {
  it("defaults ON — the accelerated (composited) renderer is the norm", () => {
    // The mask used to be gated off on Linux WebKitGTK because the webview ran
    // non-composited there. Accelerated compositing is restored, and the mask
    // measures as free, so the design intent applies on all platforms.
    expect(decideTranscriptFade(null)).toBe(true);
    expect(decideTranscriptFade(null, "accelerated")).toBe(true);
  });

  it("override 'off' disables the mask on any renderer", () => {
    expect(decideTranscriptFade("off")).toBe(false);
    expect(decideTranscriptFade("off", "accelerated")).toBe(false);
  });

  it("override 'on' keeps the mask on", () => {
    expect(decideTranscriptFade("on")).toBe(true);
  });

  it("disables the mask on the compatibility (CPU) renderer", () => {
    // No compositor to hand the mask to — it costs a full-viewport
    // re-rasterization per scroll frame there.
    expect(decideTranscriptFade(null, "compatibility")).toBe(false);
    expect(decideTranscriptFade(null, "accelerated")).toBe(true);
  });

  it("override 'on' still wins over the compatibility renderer", () => {
    expect(decideTranscriptFade("on", "compatibility")).toBe(true);
    expect(decideTranscriptFade("off", "accelerated")).toBe(false);
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
