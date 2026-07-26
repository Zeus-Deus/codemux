import { describe, it, expect } from "vitest";
import { formatCwdHint } from "./terminal-cwd";

const ROOT = "/home/zeus/projects/codemux";
const HOME = "/home/zeus";

describe("formatCwdHint", () => {
  describe("the quiet case", () => {
    it("returns null at the workspace root so the header stays unchanged", () => {
      expect(formatCwdHint(ROOT, ROOT, HOME)).toBeNull();
    });

    it("treats a trailing separator as the same directory", () => {
      expect(formatCwdHint(`${ROOT}/`, ROOT, HOME)).toBeNull();
      expect(formatCwdHint(ROOT, `${ROOT}/`, HOME)).toBeNull();
    });

    it("returns null when the cwd is not known yet", () => {
      expect(formatCwdHint(null, ROOT, HOME)).toBeNull();
      expect(formatCwdHint(undefined, ROOT, HOME)).toBeNull();
      expect(formatCwdHint("", ROOT, HOME)).toBeNull();
    });
  });

  describe("inside the workspace", () => {
    it("shows the path relative to the workspace root", () => {
      expect(formatCwdHint(`${ROOT}/src-tauri`, ROOT, HOME)?.label).toBe(
        "src-tauri",
      );
    });

    it("keeps two segments without eliding", () => {
      expect(formatCwdHint(`${ROOT}/src/components`, ROOT, HOME)?.label).toBe(
        "src/components",
      );
    });

    it("elides the head of a deep path, keeping the meaningful tail", () => {
      expect(
        formatCwdHint(`${ROOT}/src-tauri/src/pty_daemon`, ROOT, HOME)?.label,
      ).toBe("…/src/pty_daemon");
    });

    it("does not treat a sibling with a shared prefix as nested", () => {
      // `/…/codemux-old` must not be reported as `codemux` + "-old".
      const hint = formatCwdHint("/home/zeus/projects/codemux-old", ROOT, HOME);
      expect(hint?.label).toBe("~/projects/codemux-old");
    });
  });

  describe("outside the workspace", () => {
    it("contracts a $HOME-relative path to ~", () => {
      expect(formatCwdHint("/home/zeus/dotfiles", ROOT, HOME)?.label).toBe(
        "~/dotfiles",
      );
    });

    it("elides a deep $HOME-relative path", () => {
      expect(
        formatCwdHint("/home/zeus/a/b/c/notes", ROOT, HOME)?.label,
      ).toBe("~/…/c/notes");
    });

    it("renders $HOME itself as ~", () => {
      expect(formatCwdHint(HOME, ROOT, HOME)?.label).toBe("~");
    });

    it("shows a short absolute path in full", () => {
      expect(formatCwdHint("/etc", ROOT, HOME)?.label).toBe("/etc");
      expect(formatCwdHint("/", ROOT, HOME)?.label).toBe("/");
    });

    it("elides a deep absolute path outside $HOME", () => {
      expect(formatCwdHint("/var/lib/docker/volumes", ROOT, HOME)?.label).toBe(
        "…/docker/volumes",
      );
    });
  });

  describe("missing context", () => {
    it("falls back to a ~ label when the workspace root is unknown", () => {
      expect(formatCwdHint("/home/zeus/projects/x", null, HOME)?.label).toBe(
        "~/projects/x",
      );
    });

    it("falls back to an absolute tail when $HOME is unknown", () => {
      expect(formatCwdHint("/home/zeus/projects/x", null, null)?.label).toBe(
        "…/projects/x",
      );
    });
  });

  describe("tooltip", () => {
    it("preserves the untrimmed path for the hover title", () => {
      const deep = `${ROOT}/src-tauri/src/pty_daemon`;
      expect(formatCwdHint(deep, ROOT, HOME)?.full).toBe(deep);
    });
  });

  describe("windows paths", () => {
    it("normalizes backslashes when comparing against the root", () => {
      expect(
        formatCwdHint("C:\\Users\\z\\proj", "C:\\Users\\z\\proj", null),
      ).toBeNull();
    });

    it("renders a nested windows path with forward slashes", () => {
      expect(
        formatCwdHint(
          "C:\\Users\\z\\proj\\src\\lib",
          "C:\\Users\\z\\proj",
          null,
        )?.label,
      ).toBe("src/lib");
    });
  });
});
