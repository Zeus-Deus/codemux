import { describe, it, expect } from "vitest";

import {
  detectSeparator,
  isRootPath,
  parentPath,
  pathBreadcrumbs,
} from "./path-utils";

describe("detectSeparator", () => {
  it("defaults to POSIX", () => {
    expect(detectSeparator("/home/dev")).toBe("/");
    expect(detectSeparator("")).toBe("/");
  });

  it("detects Windows paths (backslash, no forward slash)", () => {
    expect(detectSeparator("C:\\Users\\dev")).toBe("\\");
  });

  it("prefers POSIX when both separators appear", () => {
    // Mixed → treat as POSIX so a stray backslash in a POSIX name doesn't
    // flip the whole path.
    expect(detectSeparator("/home/dev\\weird")).toBe("/");
  });
});

describe("isRootPath", () => {
  it("treats POSIX root as a root", () => {
    expect(isRootPath("/")).toBe(true);
    expect(isRootPath("")).toBe(true);
  });

  it("treats Windows drive roots as roots", () => {
    expect(isRootPath("C:")).toBe(true);
    expect(isRootPath("C:\\")).toBe(true);
    expect(isRootPath("D:/")).toBe(true);
  });

  it("does not treat nested paths as roots", () => {
    expect(isRootPath("/home")).toBe(false);
    expect(isRootPath("C:\\Users")).toBe(false);
  });
});

describe("parentPath", () => {
  it("walks up POSIX paths", () => {
    expect(parentPath("/home/dev/projects")).toBe("/home/dev");
    expect(parentPath("/home/dev")).toBe("/home");
    expect(parentPath("/home")).toBe("/");
  });

  it("stops at the POSIX root", () => {
    expect(parentPath("/")).toBe("/");
  });

  it("ignores a trailing separator", () => {
    expect(parentPath("/home/dev/")).toBe("/home");
  });

  it("walks up Windows paths and stops at the drive root", () => {
    expect(parentPath("C:\\Users\\dev\\projects")).toBe("C:\\Users\\dev");
    expect(parentPath("C:\\Users")).toBe("C:\\");
    expect(parentPath("C:\\")).toBe("C:\\");
  });
});

describe("pathBreadcrumbs", () => {
  it("builds cumulative POSIX crumbs from root down", () => {
    expect(pathBreadcrumbs("/home/dev/projects")).toEqual([
      { name: "/", path: "/" },
      { name: "home", path: "/home" },
      { name: "dev", path: "/home/dev" },
      { name: "projects", path: "/home/dev/projects" },
    ]);
  });

  it("returns a single root crumb for POSIX root", () => {
    expect(pathBreadcrumbs("/")).toEqual([{ name: "/", path: "/" }]);
  });

  it("ignores a trailing separator", () => {
    expect(pathBreadcrumbs("/home/dev/")).toEqual([
      { name: "/", path: "/" },
      { name: "home", path: "/home" },
      { name: "dev", path: "/home/dev" },
    ]);
  });

  it("builds Windows crumbs with the drive as root", () => {
    expect(pathBreadcrumbs("C:\\Users\\dev")).toEqual([
      { name: "C:", path: "C:\\" },
      { name: "Users", path: "C:\\Users" },
      { name: "dev", path: "C:\\Users\\dev" },
    ]);
  });

  it("every crumb path round-trips through parentPath", () => {
    const crumbs = pathBreadcrumbs("/home/dev/projects/codemux");
    for (let i = crumbs.length - 1; i > 0; i--) {
      expect(parentPath(crumbs[i].path)).toBe(crumbs[i - 1].path);
    }
  });
});
