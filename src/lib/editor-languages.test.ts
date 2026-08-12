import { describe, it, expect } from "vitest";
import {
  isBinaryExtension,
  isImageExtension,
  isVideoExtension,
} from "./editor-languages";

describe("isImageExtension", () => {
  it("returns true for common raster image extensions", () => {
    for (const f of [
      "a.png",
      "a.jpg",
      "a.jpeg",
      "a.gif",
      "a.webp",
      "a.avif",
      "a.bmp",
      "a.ico",
    ]) {
      expect(isImageExtension(f)).toBe(true);
    }
  });

  it("returns true for SVG", () => {
    expect(isImageExtension("logo.svg")).toBe(true);
  });

  it("is case-insensitive", () => {
    expect(isImageExtension("Photo.PNG")).toBe(true);
    expect(isImageExtension("Photo.JpG")).toBe(true);
  });

  it("returns false for non-image binary files", () => {
    for (const f of ["a.pdf", "a.zip", "a.mp4", "a.woff", "a.exe"]) {
      expect(isImageExtension(f)).toBe(false);
    }
  });

  it("returns false for text files", () => {
    expect(isImageExtension("README.md")).toBe(false);
    expect(isImageExtension("main.ts")).toBe(false);
  });

  it("returns false for files without a recognised extension", () => {
    expect(isImageExtension("Makefile")).toBe(false);
    expect(isImageExtension("LICENSE")).toBe(false);
    expect(isImageExtension("notes")).toBe(false);
  });

  it("uses the last extension on dotted filenames", () => {
    expect(isImageExtension("archive.png.zip")).toBe(false);
    expect(isImageExtension("backup.zip.png")).toBe(true);
  });
});

describe("isBinaryExtension", () => {
  it("still treats images as binary so other binary handling is unchanged", () => {
    // The image viewer short-circuits before this check, but every
    // other consumer of `isBinaryExtension` must keep seeing images
    // as binary or they'll start trying to read PNG bytes as UTF-8.
    expect(isBinaryExtension("a.png")).toBe(true);
    expect(isBinaryExtension("a.jpg")).toBe(true);
    expect(isBinaryExtension("a.svg")).toBe(true);
  });

  it("flags non-image binaries", () => {
    expect(isBinaryExtension("a.pdf")).toBe(true);
    expect(isBinaryExtension("a.zip")).toBe(true);
    expect(isBinaryExtension("a.mp4")).toBe(true);
    expect(isBinaryExtension("a.woff2")).toBe(true);
  });

  it("returns false for text files", () => {
    expect(isBinaryExtension("README.md")).toBe(false);
    expect(isBinaryExtension("main.rs")).toBe(false);
    expect(isBinaryExtension("Cargo.toml")).toBe(false);
  });
});

describe("isVideoExtension", () => {
  it("recognises common video containers case-insensitively", () => {
    for (const f of [
      "demo.mp4",
      "demo.m4v",
      "demo.webm",
      "demo.mov",
      "demo.ogv",
      "demo.mkv",
      "demo.AVI",
    ]) {
      expect(isVideoExtension(f)).toBe(true);
    }
  });

  it("does not classify audio, image, or text files as video", () => {
    for (const f of ["song.mp3", "sound.ogg", "photo.png", "README.md"]) {
      expect(isVideoExtension(f)).toBe(false);
    }
  });
});
