import { describe, expect, it } from "vitest";

import {
  extractToolResultImages,
  hasToolResultImages,
  isRenderableImageBlock,
} from "./tool-result-images";

describe("extractToolResultImages", () => {
  it("pulls an Anthropic base64 image block into a data: URL", () => {
    const content = [
      {
        type: "image",
        source: { type: "base64", media_type: "image/png", data: "AAAA" },
      },
    ];
    expect(extractToolResultImages(content)).toEqual([
      { src: "data:image/png;base64,AAAA", mediaType: "image/png" },
    ]);
  });

  it("defaults a missing media_type to image/png", () => {
    const content = [
      { type: "image", source: { type: "base64", data: "AAAA" } },
    ];
    expect(extractToolResultImages(content)[0]?.src).toBe(
      "data:image/png;base64,AAAA",
    );
  });

  it("handles a URL image block (http and data)", () => {
    expect(
      extractToolResultImages([
        { type: "image", source: { type: "url", url: "https://x/y.png" } },
      ])[0]?.src,
    ).toBe("https://x/y.png");
    expect(
      extractToolResultImages([
        {
          type: "image",
          source: { type: "url", url: "data:image/jpeg;base64,ZZ" },
        },
      ])[0]?.src,
    ).toBe("data:image/jpeg;base64,ZZ");
  });

  it("handles OpenAI-style image_url blocks", () => {
    expect(
      extractToolResultImages([
        { type: "image_url", image_url: { url: "https://x/y.png" } },
      ])[0]?.src,
    ).toBe("https://x/y.png");
  });

  it("keeps text blocks out and collects only images", () => {
    const content = [
      { type: "text", text: "here is the screenshot" },
      {
        type: "image",
        source: { type: "base64", media_type: "image/png", data: "BBBB" },
      },
    ];
    expect(extractToolResultImages(content)).toHaveLength(1);
  });

  it("rejects a non-image base64 media type (e.g. a PDF document)", () => {
    const content = [
      {
        type: "image",
        source: { type: "base64", media_type: "application/pdf", data: "PP" },
      },
    ];
    expect(extractToolResultImages(content)).toEqual([]);
  });

  it("rejects unsafe URL schemes", () => {
    expect(
      extractToolResultImages([
        { type: "image", source: { type: "url", url: "javascript:alert(1)" } },
      ]),
    ).toEqual([]);
    expect(
      extractToolResultImages([
        { type: "image", source: { type: "url", url: "file:///etc/passwd" } },
      ]),
    ).toEqual([]);
  });

  it("returns [] for string / empty / non-array content", () => {
    expect(extractToolResultImages("plain text")).toEqual([]);
    expect(extractToolResultImages(null)).toEqual([]);
    expect(extractToolResultImages([])).toEqual([]);
    expect(extractToolResultImages([{ type: "text", text: "x" }])).toEqual([]);
  });
});

describe("hasToolResultImages", () => {
  it("is true only when a renderable image is present", () => {
    expect(
      hasToolResultImages([
        { type: "image", source: { type: "base64", data: "AAAA" } },
      ]),
    ).toBe(true);
    expect(hasToolResultImages([{ type: "text", text: "x" }])).toBe(false);
    expect(hasToolResultImages("x")).toBe(false);
  });
});

describe("isRenderableImageBlock", () => {
  it("is true only for blocks the image renderer accepts", () => {
    expect(
      isRenderableImageBlock({
        type: "image",
        source: { type: "base64", media_type: "image/png", data: "AAAA" },
      }),
    ).toBe(true);
    expect(
      isRenderableImageBlock({
        type: "image_url",
        image_url: { url: "https://example.com/screenshot.png" },
      }),
    ).toBe(true);
    expect(isRenderableImageBlock({ type: "image" })).toBe(false);
    expect(
      isRenderableImageBlock({
        type: "image",
        source: {
          type: "base64",
          media_type: "application/pdf",
          data: "PP",
        },
      }),
    ).toBe(false);
    expect(
      isRenderableImageBlock({
        type: "image_url",
        image_url: { url: "javascript:alert(1)" },
      }),
    ).toBe(false);
    expect(isRenderableImageBlock({ type: "text", text: "x" })).toBe(false);
    expect(isRenderableImageBlock("x")).toBe(false);
  });
});
