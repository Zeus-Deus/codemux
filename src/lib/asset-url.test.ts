import { describe, it, expect, vi } from "vitest";

// Mock convertFileSrc so the test runs outside Tauri and we can
// observe the exact path the resolver would have handed to the
// asset protocol.
vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => `mock-asset:${path}`,
}));

import {
  dirname,
  isAbsoluteFsPath,
  isRemoteOrDataUrl,
  joinPath,
  resolveAssetSrc,
} from "./asset-url";

describe("isRemoteOrDataUrl", () => {
  it.each([
    "https://example.com/x.png",
    "http://example.com/x.png",
    "data:image/png;base64,AAA",
    "blob:https://example.com/abc",
    "asset://localhost/foo",
    "tauri://localhost/foo",
  ])("returns true for %s", (url) => {
    expect(isRemoteOrDataUrl(url)).toBe(true);
  });

  it.each([
    "/abs/path.png",
    "./relative.png",
    "../sibling.png",
    "no-scheme.png",
    "C:\\Users\\me\\x.png",
  ])("returns false for %s", (path) => {
    expect(isRemoteOrDataUrl(path)).toBe(false);
  });
});

describe("isAbsoluteFsPath", () => {
  it("recognises POSIX absolute paths", () => {
    expect(isAbsoluteFsPath("/foo/bar.png")).toBe(true);
    expect(isAbsoluteFsPath("/")).toBe(true);
  });

  it("recognises Windows drive paths", () => {
    expect(isAbsoluteFsPath("C:\\foo\\bar.png")).toBe(true);
    expect(isAbsoluteFsPath("D:/foo/bar.png")).toBe(true);
  });

  it("rejects relative paths and remote URLs", () => {
    expect(isAbsoluteFsPath("./foo.png")).toBe(false);
    expect(isAbsoluteFsPath("foo.png")).toBe(false);
    expect(isAbsoluteFsPath("../foo.png")).toBe(false);
    expect(isAbsoluteFsPath("https://example.com/x.png")).toBe(false);
  });
});

describe("dirname", () => {
  it("returns the parent directory for POSIX paths", () => {
    expect(dirname("/a/b/c.md")).toBe("/a/b");
  });

  it("returns the parent directory for Windows paths", () => {
    expect(dirname("C:\\a\\b\\c.md")).toBe("C:\\a\\b");
  });

  it("returns empty string when there is no separator", () => {
    expect(dirname("c.md")).toBe("");
  });
});

describe("joinPath", () => {
  it("joins POSIX directory + relative file", () => {
    expect(joinPath("/a/b", "c.png")).toBe("/a/b/c.png");
  });

  it("strips a leading ./ from the relative path", () => {
    expect(joinPath("/a/b", "./c.png")).toBe("/a/b/c.png");
  });

  it("preserves ../ segments for the OS to resolve", () => {
    expect(joinPath("/a/b", "../c.png")).toBe("/a/b/../c.png");
  });

  it("uses backslash separator for Windows-shaped dirs", () => {
    expect(joinPath("C:\\a\\b", "c.png")).toBe("C:\\a\\b\\c.png");
  });

  it("returns the relative path verbatim when dir is empty", () => {
    expect(joinPath("", "c.png")).toBe("c.png");
    expect(joinPath("", "./c.png")).toBe("c.png");
  });
});

describe("resolveAssetSrc", () => {
  it("returns undefined when src is undefined", () => {
    expect(resolveAssetSrc(undefined, "/a/b.md")).toBeUndefined();
  });

  it("passes remote URLs through unchanged", () => {
    expect(resolveAssetSrc("https://example.com/x.png", "/a/b.md")).toBe(
      "https://example.com/x.png",
    );
    expect(resolveAssetSrc("data:image/png;base64,xxx", "/a/b.md")).toBe(
      "data:image/png;base64,xxx",
    );
  });

  it("converts absolute POSIX paths via convertFileSrc", () => {
    expect(resolveAssetSrc("/abs/path/x.png", "/a/b.md")).toBe(
      "mock-asset:/abs/path/x.png",
    );
  });

  it("converts absolute Windows paths via convertFileSrc", () => {
    expect(resolveAssetSrc("C:\\abs\\x.png", "C:\\a\\b.md")).toBe(
      "mock-asset:C:\\abs\\x.png",
    );
  });

  it("resolves relative paths against the markdown file directory", () => {
    expect(resolveAssetSrc("./img/x.png", "/docs/guide.md")).toBe(
      "mock-asset:/docs/img/x.png",
    );
  });

  it("resolves bare relative paths (no leading ./)", () => {
    expect(resolveAssetSrc("img/x.png", "/docs/guide.md")).toBe(
      "mock-asset:/docs/img/x.png",
    );
  });

  it("preserves ../ segments in relative paths", () => {
    expect(resolveAssetSrc("../assets/x.png", "/docs/guide.md")).toBe(
      "mock-asset:/docs/../assets/x.png",
    );
  });

  it("returns the raw src when relative and no base path is given", () => {
    expect(resolveAssetSrc("./x.png", null)).toBe("./x.png");
    expect(resolveAssetSrc("./x.png", undefined)).toBe("./x.png");
  });
});
