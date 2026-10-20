import { describe, expect, it } from "vitest";

import nativeImageResultRaw from "./__fixtures__/opencode-native-image-tool-result.json?raw";
import {
  extractToolResultImages,
  hasToolResultImages,
  isRenderableImageBlock,
  withoutToolResultImages,
  toolResultImagePointers,
} from "./tool-result-images";

describe("extractToolResultImages", () => {
  it("renders the native image file payload preserved by the OpenCode adapter", () => {
    const fixture: { data: { content: unknown[] } } =
      JSON.parse(nativeImageResultRaw);
    const images = extractToolResultImages(fixture.data.content);
    expect(images).toHaveLength(1);
    expect(images[0]).toMatchObject({ mediaType: "image/png" });
    expect(images[0]?.src).toMatch(/^data:image\/png;base64,/);
    expect(isRenderableImageBlock(fixture.data.content[1])).toBe(true);
    expect(hasToolResultImages(fixture.data.content)).toBe(true);
  });

  it.each([
    { type: "file", mime: "image/png", uri: "javascript:alert(1)" },
    { type: "file", mime: "image/png", uri: "file:///etc/passwd" },
    { type: "file", mime: "image/png", uri: "data:text/html;base64,AAAA" },
    { type: "file", mime: "application/pdf", uri: "https://example.com/x.pdf" },
    { type: "file", mime: "image/png" },
  ])(
    "keeps unsafe or non-image native file blocks in the text fallback: %j",
    (block) => {
      expect(extractToolResultImages([block])).toEqual([]);
      expect(isRenderableImageBlock(block)).toBe(false);
    },
  );

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

  it("extracts native result envelopes without scanning arbitrary tool inputs", () => {
    const image = { type: "image", source: { type: "base64", data: "A".repeat(40_000) } };
    for (const [content, pointer] of [
      [{ type: "mcpToolCall", result: { content: [{ type: "text", text: "captured" }, image] } }, "/result/content/1"],
      [{ type: "dynamicToolCall", contentItems: [image] }, "/contentItems/0"],
    ] as const) {
      expect(extractToolResultImages(content)).toHaveLength(1);
      expect(toolResultImagePointers(content)).toEqual([{ pointer }]);
      expect(JSON.stringify(withoutToolResultImages(content))).not.toContain("AAAA");
      expect(JSON.stringify(content)).toContain("AAAA");
    }
    expect(extractToolResultImages({ input: { content: [image] } })).toEqual([]);
  });

  it("keeps small, unsupported and parameterized sources inline in the mock projection", () => {
    for (const src of ["https://example.test/a.png", `data:image/svg+xml;base64,${"A".repeat(40_000)}`, `data:image/avif;base64,${"A".repeat(40_000)}`, `data:image/png;charset=utf-8;base64,${"A".repeat(40_000)}`]) {
      expect(toolResultImagePointers([{ type: "image_url", image_url: src }])).toEqual([{ pointer: "/0", src }]);
    }
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
