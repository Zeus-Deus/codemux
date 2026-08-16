import { describe, expect, it } from "vitest";

import { resolveExistingFileTarget } from "./file-link-target";
import type { ChatFileLinkMeta } from "./file-links";

const meta = (filePath: string): ChatFileLinkMeta => ({
  filePath,
  basename: filePath.split("/").pop() ?? filePath,
  displayPath: filePath,
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
});
