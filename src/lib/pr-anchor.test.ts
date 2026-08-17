import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import {
  anchorContext,
  indexDiffRows,
  reanchor,
  rowsFor,
  type DraftAnchor,
} from "./pr-anchor";

/**
 * The captured artefacts of a real force-push on a real repository.
 *
 * `anchors.json` are the four draft anchors written against
 * `diff-before.patch`; `diff-after.patch` is the diff GitHub served
 * after the branch was rewritten. The verdicts asserted below are the
 * ones the live experiment produced — GitHub's own re-anchoring agreed
 * with all four. If this test ever goes red, the matcher has stopped
 * matching the host, and notes will be submitted against lines their
 * authors never read.
 */
const fixture = (name: string) =>
  readFileSync(resolve(process.cwd(), "src/lib/__fixtures__", name), "utf8");

const before = fixture("diff-before.patch");
const after = fixture("diff-after.patch");
const anchors = JSON.parse(fixture("anchors.json")) as Array<{
  path: string;
  side: "LEFT" | "RIGHT";
  line: number;
  text: string;
  context_before?: string;
  context_after?: string;
}>;

const asAnchor = (a: (typeof anchors)[number]): DraftAnchor => ({
  path: a.path,
  side: a.side,
  line: a.line,
  text: a.text,
  contextBefore: a.context_before ?? null,
  contextAfter: a.context_after ?? null,
});

/**
 * The same diffs, as a Windows checkout hands them over.
 *
 * `core.autocrlf=true` rewrites these fixtures on the way to the working
 * tree, and `gh` on a Windows host can produce the same thing at
 * runtime. Every decision the parser makes is on the exact characters of
 * a line — the `+`/`-` prefixes, the `@@` header, the file paths that
 * become index keys, and a blank context line that has to be `""` — so
 * these assert the CRLF form indexes to *precisely* the LF form rather
 * than merely producing something.
 *
 * `.gitattributes` pins `*.patch` to LF so the fixtures stop being
 * rewritten at all; this is the second layer, for diff text that was
 * never a file here.
 */
describe("a diff that arrives with CRLF", () => {
  const crlf = (text: string) => text.replace(/\r?\n/g, "\r\n");

  it("indexes identically to the same diff with LF", () => {
    const lf = indexDiffRows(before);
    const windows = indexDiffRows(crlf(before));

    expect(lf.size).toBeGreaterThan(0);
    expect([...windows.keys()].sort()).toEqual([...lf.keys()].sort());
    for (const key of lf.keys()) {
      expect(windows.get(key)).toEqual(lf.get(key));
    }
  });

  it("reaches the same re-anchoring verdicts", () => {
    const lf = indexDiffRows(after);
    const windows = indexDiffRows(crlf(after));
    for (const raw of anchors) {
      expect(reanchor(asAnchor(raw), windows)).toEqual(reanchor(asAnchor(raw), lf));
    }
  });

  it("still reads a blank context line as blank context", () => {
    // The mechanism that broke: a blank context line is `""` under LF and
    // `"\r"` under CRLF, which fell through to "anything else ends the
    // hunk" — so the hunk body stopped early and every row below it went
    // unindexed.
    const diff = crlf(
      [
        "diff --git a/a.txt b/a.txt",
        "--- a/a.txt",
        "+++ b/a.txt",
        "@@ -1,3 +1,3 @@",
        "-one",
        "+ONE",
        "",
        " three",
        "",
      ].join("\n"),
    );

    const right = rowsFor(indexDiffRows(diff), "a.txt", "RIGHT");
    expect(right.map((r) => ({ line: r.line, text: r.text }))).toEqual([
      { line: 1, text: "ONE" },
      { line: 2, text: "" },
      { line: 3, text: "three" },
    ]);
  });
});

describe("indexDiffRows", () => {
  it("numbers RIGHT rows by the new file and LEFT rows by the old", () => {
    const index = indexDiffRows(before);
    const right = rowsFor(index, "app.py", "RIGHT");
    const left = rowsFor(index, "app.py", "LEFT");

    // The first hunk starts at old 2 / new 2 with three context lines.
    expect(right[0]).toMatchObject({ line: 2, text: "def func_02(): return 'value_02'  # region A" });
    expect(left[0]).toMatchObject({ line: 2, text: "def func_02(): return 'value_02'  # region A" });

    // The added region-A rewrite lands on the RIGHT at its new numbers.
    expect(right.find((r) => r.line === 5)?.text).toBe(
      "def func_05(): return compute('value_05')  # region A modified",
    );
    // …and the line it replaced stays addressable on the LEFT.
    expect(left.find((r) => r.line === 5)?.text).toBe(
      "def func_05(): return 'value_05'  # region A",
    );
  });

  it("ignores preamble text before the first file header", () => {
    // The captured patches begin with a `mise` tool banner.
    expect(before.split("\n")[0]).toContain("mise");
    expect(rowsFor(indexDiffRows(before), "app.py", "RIGHT").length).toBeGreaterThan(0);
  });

  it("does not invent a row past the end of a hunk", () => {
    // A diff that ends in a newline splits into a trailing `""`, and the
    // blank-context rule used to index that empty string as a real row —
    // one past the last line of the hunk, on both sides at once. The
    // damage was two-fold: re-anchoring could land a note on a line the
    // file does not have (the host answers 422), and the true last row
    // reported `contextAfter: ""` instead of `null`, which disqualified
    // the correct candidate whenever `reanchor` had to break a tie.
    const diff = [
      "diff --git a/a.txt b/a.txt",
      "--- a/a.txt",
      "+++ b/a.txt",
      "@@ -1,2 +1,2 @@",
      "-one",
      "+ONE",
      " two",
      "",
    ].join("\n");

    const index = indexDiffRows(diff);
    const right = rowsFor(index, "a.txt", "RIGHT");
    const left = rowsFor(index, "a.txt", "LEFT");

    expect(right.map((r) => r.line)).toEqual([1, 2]);
    expect(left.map((r) => r.line)).toEqual([1, 2]);
    expect(anchorContext(index, "a.txt", "RIGHT", 3)).toBeNull();
    // The last real row is last: nothing follows it.
    expect(anchorContext(index, "a.txt", "RIGHT", 2)?.contextAfter).toBeNull();
  });

  it("still counts a blank line inside a hunk body as context", () => {
    // The trailing-newline fix pops exactly one empty string. An interior
    // blank is a real row, and dropping it would renumber everything
    // below it.
    const diff = [
      "diff --git a/a.txt b/a.txt",
      "--- a/a.txt",
      "+++ b/a.txt",
      "@@ -1,3 +1,3 @@",
      "-one",
      "+ONE",
      "",
      " three",
      "",
    ].join("\n");

    const right = rowsFor(indexDiffRows(diff), "a.txt", "RIGHT");
    expect(right.map((r) => ({ line: r.line, text: r.text }))).toEqual([
      { line: 1, text: "ONE" },
      { line: 2, text: "" },
      { line: 3, text: "three" },
    ]);
  });

  it("reads a line's neighbours and hunk straight off the diff", () => {
    const ctx = anchorContext(indexDiffRows(before), "app.py", "RIGHT", 47);
    expect(ctx).toEqual({
      text: "def func_45(): return cached('value_45')  # region C modified",
      contextBefore: "def func_44(): return 'value_44'  # region C",
      contextAfter: "def func_46(): return cached('value_46')  # region C modified",
      hunk: "@@ -42,10 +44,10 @@ def func_41(): return 'value_41'  # region C",
    });
  });
});

describe("reanchor against the captured force-push", () => {
  const index = indexDiffRows(after);
  const byLineAndSide = (line: number, side: "LEFT" | "RIGHT") =>
    asAnchor(anchors.find((a) => a.line === line && a.side === side)!);

  it("moves the surviving region-C note from 47 to 50", () => {
    expect(reanchor(byLineAndSide(47, "RIGHT"), index)).toEqual({
      status: "reanchored",
      line: 50,
      hunk: "@@ -42,10 +47,10 @@ def func_41(): return 'value_41'  # region C",
      moved: true,
    });
  });

  it("loses the note on the line that was rewritten", () => {
    // `compute(…)` became `compute_v2(…, retries=3)`. The text is gone,
    // so the note is kept and labelled rather than guessed onto a
    // neighbour.
    expect(reanchor(byLineAndSide(5, "RIGHT"), index)).toEqual({
      status: "lost",
      reason: "line text absent from new diff",
    });
  });

  it("moves a two-line selection from 21–22 to 24–25", () => {
    // A range anchors on both ends; both must survive for the range to.
    expect(reanchor(byLineAndSide(21, "RIGHT"), index)).toMatchObject({
      status: "reanchored",
      line: 24,
      moved: true,
    });
    const end: DraftAnchor = {
      path: "app.py",
      side: "RIGHT",
      line: 22,
      text: "def helper_b2(): return 'b2'  # new helper",
      contextBefore: "def helper_b1(): return 'b1'  # new helper",
      contextAfter: "def func_21(): return 'value_21'  # region B",
    };
    expect(reanchor(end, index)).toMatchObject({
      status: "reanchored",
      line: 25,
      moved: true,
    });
  });

  it("leaves the LEFT-side note at 5 — the base didn't move", () => {
    expect(reanchor(byLineAndSide(5, "LEFT"), index)).toEqual({
      status: "reanchored",
      line: 5,
      hunk: "@@ -1,11 +1,14 @@",
      moved: false,
    });
  });

  it("agrees with the prototype on every captured anchor", () => {
    const verdicts = anchors.map((a) => {
      const r = reanchor(asAnchor(a), index);
      return r.status === "reanchored" ? `${a.side}:${a.line}->${r.line}` : `${a.side}:${a.line}->lost`;
    });
    expect(verdicts).toEqual([
      "RIGHT:47->50",
      "RIGHT:5->lost",
      "RIGHT:21->24",
      "LEFT:5->5",
    ]);
  });
});

describe("reanchor safety rules", () => {
  const diff = [
    "diff --git a/dup.ts b/dup.ts",
    "--- a/dup.ts",
    "+++ b/dup.ts",
    "@@ -1,6 +1,6 @@",
    " head",
    "+  return null;",
    " middle",
    "+  return null;",
    " tail",
  ].join("\n");

  it("reports a distance tie as lost rather than guessing", () => {
    const index = indexDiffRows(diff);
    // Both `return null;` rows sit at new lines 2 and 4; an anchor
    // remembered at 3 is exactly one away from each.
    const tie: DraftAnchor = {
      path: "dup.ts",
      side: "RIGHT",
      line: 3,
      text: "  return null;",
      contextBefore: null,
      contextAfter: null,
    };
    expect(reanchor(tie, index)).toEqual({
      status: "lost",
      reason: "ambiguous: multiple equal matches",
    });
  });

  it("breaks a duplicate-text tie with the neighbouring lines", () => {
    const index = indexDiffRows(diff);
    const withContext: DraftAnchor = {
      path: "dup.ts",
      side: "RIGHT",
      line: 3,
      text: "  return null;",
      contextBefore: "middle",
      contextAfter: "tail",
    };
    expect(reanchor(withContext, index)).toMatchObject({ status: "reanchored", line: 4 });
  });

  it("loses an anchor whose file left the diff entirely", () => {
    const gone: DraftAnchor = {
      path: "removed.ts",
      side: "RIGHT",
      line: 1,
      text: "anything",
    };
    expect(reanchor(gone, indexDiffRows(diff))).toMatchObject({ status: "lost" });
  });
});
