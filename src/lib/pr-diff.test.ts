import { describe, it, expect } from "vitest";
import {
  changedLines,
  ignoreWhitespace,
  isGeneratedPath,
  isLargeFile,
  isRenderable,
  splitDiffFiles,
  LARGE_FILE_CHANGED_LINES,
} from "./pr-diff";
import { parseDiff } from "./diff-parser";

const SMALL = [
  "diff --git a/src/a.ts b/src/a.ts",
  "index 111..222 100644",
  "--- a/src/a.ts",
  "+++ b/src/a.ts",
  "@@ -10,3 +10,4 @@ header",
  " const a = 1;",
  "-const b = 2;",
  "+const b = 3;",
  "+const c = 4;",
  " const d = 5;",
].join("\n");

const BINARY = [
  "diff --git a/icons/app.png b/icons/app.png",
  "index 333..444 100644",
  "Binary files a/icons/app.png and b/icons/app.png differ",
].join("\n");

const DELETED = [
  "diff --git a/src/old.ts b/src/old.ts",
  "deleted file mode 100644",
  "index 555..0000000",
  "--- a/src/old.ts",
  "+++ /dev/null",
  "@@ -1,2 +0,0 @@",
  "-export const gone = true;",
  "-export default gone;",
].join("\n");

function huge(): string {
  const rows: string[] = [];
  for (let i = 0; i < 1_500; i++) rows.push(`-const x${i} = ${i};`);
  for (let i = 0; i < 1_500; i++) rows.push(`+const x${i} = ${i + 1};`);
  return [
    "diff --git a/src/big.ts b/src/big.ts",
    "--- a/src/big.ts",
    "+++ b/src/big.ts",
    "@@ -1,1500 +1,1500 @@",
    ...rows,
  ].join("\n");
}

describe("splitDiffFiles", () => {
  it("cuts a multi-file diff into files with their own stats", () => {
    const files = splitDiffFiles([SMALL, BINARY, DELETED].join("\n"));
    expect(files.map((f) => f.path)).toEqual([
      "src/a.ts",
      "icons/app.png",
      "src/old.ts",
    ]);
    expect(files[0]).toMatchObject({ additions: 2, deletions: 1, status: "modified" });
    expect(files[2]).toMatchObject({ status: "deleted", deletions: 2, additions: 0 });
  });

  it("drops a tool banner printed before the first file header", () => {
    const files = splitDiffFiles(`mise tools: gh@2.97.0\n${SMALL}`);
    expect(files).toHaveLength(1);
    expect(files[0].lines).toEqual(parseDiff(files[0].patch));
  });

  it("marks a binary file and gives it no rows to click", () => {
    const [file] = splitDiffFiles(BINARY);
    expect(file.binary).toBe(true);
    expect(file.lines).toEqual([]);
    expect(isRenderable(file)).toBe(false);
  });

  it("names a deleted file by its old path", () => {
    const [file] = splitDiffFiles(DELETED);
    expect(file.path).toBe("src/old.ts");
    expect(file.oldPath).toBe("src/old.ts");
  });

  it("returns nothing for an empty diff", () => {
    expect(splitDiffFiles("")).toEqual([]);
  });
});

describe("size thresholds", () => {
  it("holds back a file past the changed-line threshold", () => {
    const [file] = splitDiffFiles(huge());
    expect(changedLines(file)).toBe(3_000);
    expect(changedLines(file)).toBeGreaterThan(LARGE_FILE_CHANGED_LINES);
    expect(isLargeFile(file)).toBe(true);
  });

  it("leaves an ordinary file alone", () => {
    const [file] = splitDiffFiles(SMALL);
    expect(isLargeFile(file)).toBe(false);
    expect(isRenderable(file)).toBe(true);
  });

  it("treats lockfiles and minified bundles as generated", () => {
    expect(isGeneratedPath("package-lock.json")).toBe(true);
    expect(isGeneratedPath("apps/web/pnpm-lock.yaml")).toBe(true);
    expect(isGeneratedPath("dist/app.min.js")).toBe(true);
    expect(isGeneratedPath("src/lock-manager.ts")).toBe(false);
  });

  it("never calls a binary file large — it has no lines to count", () => {
    const [file] = splitDiffFiles(BINARY);
    expect(isLargeFile(file)).toBe(false);
  });
});

describe("ignoreWhitespace", () => {
  it("folds a spacing-only rewrite back into context, numbers intact", () => {
    const lines = parseDiff(
      [
        "@@ -1,2 +1,2 @@",
        "-  const a = 1;",
        "+    const a = 1;",
        " const b = 2;",
      ].join("\n"),
    );
    const folded = ignoreWhitespace(lines);
    const rows = folded.filter((l) => l.type !== "hunk-header");
    expect(rows.map((l) => l.type)).toEqual(["context", "context"]);
    expect(rows[0]).toMatchObject({ oldLine: 1, newLine: 1, content: "    const a = 1;" });
  });

  it("leaves a real edit alone even when it shares a block", () => {
    const lines = parseDiff(
      ["@@ -1,2 +1,2 @@", "-  const a = 1;", "-const b = 2;", "+    const a = 1;"].join(
        "\n",
      ),
    );
    // Two deletions against one addition is not a pure reindent.
    const types = ignoreWhitespace(lines)
      .filter((l) => l.type !== "hunk-header")
      .map((l) => l.type);
    expect(types).toEqual(["del", "del", "add"]);
  });
});

describe("a diff produced on a Windows host", () => {
  it("splits CRLF into exactly the same files, paths and stats", () => {
    const lf = [SMALL, BINARY, DELETED].join("\n");
    const crlf = lf.replace(/\r?\n/g, "\r\n");

    // Deep-equal, so a \r surviving into a path, a rename or a line of
    // the per-file patch fails here rather than on a Windows desk.
    expect(splitDiffFiles(crlf)).toEqual(splitDiffFiles(lf));
    expect(splitDiffFiles(crlf).map((f) => f.path)).toEqual([
      "src/a.ts",
      "icons/app.png",
      "src/old.ts",
    ]);
  });
});
