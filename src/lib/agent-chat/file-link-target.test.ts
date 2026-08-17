import { describe, expect, it } from "vitest";

import { resolveExistingFileTarget } from "./file-link-target";
import type { ChatFileLinkMeta } from "./file-links";

const meta = (
  filePath: string,
  workspacePath?: string,
): ChatFileLinkMeta => ({
  filePath,
  basename: filePath.split("/").pop() ?? filePath,
  displayPath: filePath,
  ...(workspacePath ? { workspacePath } : {}),
});

const existsIn =
  (present: readonly string[]) =>
  async (path: string): Promise<boolean> =>
    present.includes(path);

describe("resolveExistingFileTarget", () => {
  it("returns the resolved path when it exists", async () => {
    const target = await resolveExistingFileTarget(
      meta("/repo/src/app.ts"),
      ["/tmp/other/app.ts"],
      existsIn(["/repo/src/app.ts", "/tmp/other/app.ts"]),
    );
    expect(target).toBe("/repo/src/app.ts");
  });

  it("falls back to a turn path whose basename matches", async () => {
    const target = await resolveExistingFileTarget(
      meta("/repo/shot.png"),
      ["/tmp/spec/screenshots/shot.png"],
      existsIn(["/tmp/spec/screenshots/shot.png"]),
    );
    expect(target).toBe("/tmp/spec/screenshots/shot.png");
  });

  it("prefers the most recent matching mention", async () => {
    const target = await resolveExistingFileTarget(
      meta("/repo/shot.png"),
      ["/tmp/old/shot.png", "/tmp/new/shot.png"],
      existsIn(["/tmp/old/shot.png", "/tmp/new/shot.png"]),
    );
    expect(target).toBe("/tmp/new/shot.png");
  });

  it("skips fallback candidates that do not exist or do not match", async () => {
    const target = await resolveExistingFileTarget(
      meta("/repo/shot.png"),
      ["/tmp/missing/shot.png", "/tmp/spec/other.png"],
      existsIn(["/tmp/spec/other.png"]),
    );
    expect(target).toBeNull();
  });

  it("returns null when nothing exists", async () => {
    const target = await resolveExistingFileTarget(
      meta("/repo/shot.png"),
      [],
      existsIn([]),
    );
    expect(target).toBeNull();
  });

  it("falls back to the workspace root when the tool ran in a subdirectory", async () => {
    // A command ran with workdir `<repo>/src-tauri`, so the chip resolved
    // there, but the answer meant the repo-root file.
    const target = await resolveExistingFileTarget(
      meta("/repo/src-tauri/package.json", "/repo/package.json"),
      [],
      existsIn(["/repo/package.json"]),
    );
    expect(target).toBe("/repo/package.json");
  });

  it("prefers the tool-directory resolution over the workspace one", async () => {
    const target = await resolveExistingFileTarget(
      meta("/repo/src-tauri/Cargo.toml", "/repo/Cargo.toml"),
      [],
      existsIn(["/repo/src-tauri/Cargo.toml", "/repo/Cargo.toml"]),
    );
    expect(target).toBe("/repo/src-tauri/Cargo.toml");
  });

  it("tries the workspace path before turn paths", async () => {
    const target = await resolveExistingFileTarget(
      meta("/other/project/notes.md", "/repo/notes.md"),
      ["/tmp/scratch/notes.md"],
      existsIn(["/repo/notes.md", "/tmp/scratch/notes.md"]),
    );
    expect(target).toBe("/repo/notes.md");
  });

  it("opens the parsed path when the existence probe is unavailable", async () => {
    const target = await resolveExistingFileTarget(
      meta("/repo/src/app.ts"),
      [],
      async () => {
        throw new Error("file_exists unsupported");
      },
    );
    expect(target).toBe("/repo/src/app.ts");
  });

  it("does not open a confirmed-missing path when a later probe fails", async () => {
    const target = await resolveExistingFileTarget(
      meta("/repo/shot.png"),
      ["/tmp/spec/shot.png"],
      async (path) => {
        if (path === "/repo/shot.png") return false;
        throw new Error("probe failed");
      },
    );
    expect(target).toBeNull();
  });
});
