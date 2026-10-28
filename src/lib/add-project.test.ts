import { describe, expect, it } from "vitest";
import {
  cloneFolderName,
  defaultProjectsLocation,
  joinPath,
  normalizeCloneUrl,
  projectNameError,
} from "./add-project";

describe("defaultProjectsLocation", () => {
  it("is Projects under the home dir", () => {
    expect(defaultProjectsLocation("/home/user")).toBe("/home/user/Projects");
    expect(defaultProjectsLocation("C:\\Users\\user")).toBe("C:\\Users\\user\\Projects");
  });

  it("falls back to the tilde form the backend expands", () => {
    expect(defaultProjectsLocation(null)).toBe("~/Projects");
  });
});

describe("joinPath", () => {
  it("does not double the separator", () => {
    expect(joinPath("/home/user/Code/", "app")).toBe("/home/user/Code/app");
    expect(joinPath("/", "app")).toBe("/app");
  });
});

describe("normalizeCloneUrl", () => {
  it("expands owner/repo to GitHub https", () => {
    expect(normalizeCloneUrl("acme/field-notes")).toBe(
      "https://github.com/acme/field-notes.git",
    );
    expect(normalizeCloneUrl(" acme/field-notes.git ")).toBe(
      "https://github.com/acme/field-notes.git",
    );
  });

  it("keeps real URLs as typed", () => {
    expect(normalizeCloneUrl("https://gitlab.com/a/b.git")).toBe("https://gitlab.com/a/b.git");
    expect(normalizeCloneUrl("git@github.com:acme/app.git")).toBe("git@github.com:acme/app.git");
    expect(normalizeCloneUrl("ssh://git@host:2222/a/b")).toBe("ssh://git@host:2222/a/b");
  });

  it("adds https to a scheme-less host path", () => {
    expect(normalizeCloneUrl("github.com/acme/app")).toBe("https://github.com/acme/app");
  });

  it("rejects things that are not repositories", () => {
    expect(normalizeCloneUrl("")).toBeNull();
    expect(normalizeCloneUrl("hello")).toBeNull();
    expect(normalizeCloneUrl("a b/c")).toBeNull();
  });
});

describe("cloneFolderName", () => {
  it("takes the last segment without .git", () => {
    expect(cloneFolderName("https://github.com/acme/field-notes.git")).toBe("field-notes");
    expect(cloneFolderName("git@github.com:acme/app.git")).toBe("app");
    expect(cloneFolderName("acme/parser/")).toBe("parser");
    expect(cloneFolderName("")).toBe("");
  });
});

describe("projectNameError", () => {
  it("accepts a single folder name", () => {
    expect(projectNameError("weekend-habits")).toBeNull();
  });

  it("refuses empty names, dot names and paths", () => {
    expect(projectNameError("  ")).not.toBeNull();
    expect(projectNameError("..")).not.toBeNull();
    expect(projectNameError("a/b")).not.toBeNull();
    expect(projectNameError("a\\b")).not.toBeNull();
  });
});
