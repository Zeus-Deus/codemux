import { describe, expect, it } from "vitest";

import {
  CHAT_LOCAL_IMAGE_TAG,
  localImagePath,
  rehypeLocalImageLinks,
} from "./local-image-links";

describe("localImagePath", () => {
  it.each([
    ["/tmp/proof.png", "/tmp/proof.png"],
    ["/tmp/proof%20one.JPG", "/tmp/proof one.JPG"],
    ["C:/Users/me/proof.webp", "C:/Users/me/proof.webp"],
    ["file:///home/me/proof.gif", "/home/me/proof.gif"],
    ["file:///C:/Users/me/proof.jpeg", "C:/Users/me/proof.jpeg"],
  ])("accepts a supported absolute image path: %s", (input, expected) => {
    expect(localImagePath(input)).toBe(expected);
  });

  it.each([
    "https://example.com/proof.png",
    "./proof.png",
    "/tmp/notes.txt",
    "file://other-host/tmp/proof.png",
    "javascript:/tmp/proof.png",
  ])("rejects a non-local or unsupported destination: %s", (input) => {
    expect(localImagePath(input)).toBeNull();
  });
});

describe("rehypeLocalImageLinks", () => {
  it("upgrades a normal Markdown link while preserving its label children", () => {
    const tree = {
      type: "root",
      children: [
        {
          type: "element",
          tagName: "a",
          properties: { href: "/tmp/proof.png" },
          children: [
            { type: "element", tagName: "strong", properties: {}, children: [
              { type: "text", value: "Visual proof" },
            ] },
          ],
        },
      ],
    };

    rehypeLocalImageLinks()(tree);

    expect(tree.children[0]).toMatchObject({
      tagName: CHAT_LOCAL_IMAGE_TAG,
      properties: {
        path: "/tmp/proof.png",
        caption: "Visual proof",
        sourceSyntax: "link",
      },
    });
    expect(tree.children[0].children[0].tagName).toBe("strong");
  });

  it("upgrades absolute Markdown image syntax", () => {
    const tree = {
      type: "root",
      children: [
        {
          type: "element",
          tagName: "img",
          properties: { src: "/tmp/proof.png", alt: "Terminal result" },
          children: [],
        },
      ],
    };

    rehypeLocalImageLinks()(tree);

    expect(tree.children[0]).toEqual({
      type: "element",
      tagName: CHAT_LOCAL_IMAGE_TAG,
      properties: {
        path: "/tmp/proof.png",
        caption: "Terminal result",
        sourceSyntax: "image",
      },
      children: [],
    });
  });
});
