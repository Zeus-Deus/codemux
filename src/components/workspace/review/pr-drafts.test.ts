import { describe, it, expect, beforeEach } from "vitest";
import {
  addLineDraft,
  clearLineDrafts,
  draftCounts,
  draftKey,
  getDiffSnapshot,
  getLineDrafts,
  putDiffSnapshot,
  reanchorLineDrafts,
  removeLineDraft,
  submitBlockedReason,
  updateLineDraft,
  _resetPrDrafts,
  type NewLineDraft,
} from "./pr-drafts";

const KEY_A = draftKey("ws-1", 172);
const KEY_B = draftKey("ws-1", 285);
const OID = "aaa111";
const NEW_OID = "bbb222";

const BEFORE = [
  "diff --git a/src/a.ts b/src/a.ts",
  "--- a/src/a.ts",
  "+++ b/src/a.ts",
  "@@ -10,3 +10,4 @@ header",
  " const a = 1;",
  "-const b = 2;",
  "+const b = 3;",
  "+const c = 4;",
  " const d = 5;",
].join("\n");

/** The same file with two lines inserted above the hunk's changes and
 *  `const c` rewritten: one note must move, one must be lost. */
const AFTER = [
  "diff --git a/src/a.ts b/src/a.ts",
  "--- a/src/a.ts",
  "+++ b/src/a.ts",
  "@@ -10,3 +10,6 @@ header",
  " const a = 1;",
  "+const zero = 0;",
  "+const half = 0.5;",
  "-const b = 2;",
  "+const b = 3;",
  "+const c = 4 as const;",
  " const d = 5;",
].join("\n");

function note(over: Partial<NewLineDraft> = {}): NewLineDraft {
  return {
    path: "src/a.ts",
    side: "RIGHT",
    line: 11,
    startLine: null,
    lineText: "const b = 3;",
    startLineText: null,
    contextBefore: "const a = 1;",
    contextAfter: "const c = 4;",
    hunkHeader: "@@ -10,3 +10,4 @@ header",
    headOidAtDraft: OID,
    body: "Prefer a named constant here.",
    ...over,
  };
}

beforeEach(() => {
  _resetPrDrafts();
});

describe("the pending review survives everything but a quit", () => {
  it("keeps notes across PR switches and comes back to the right ones", () => {
    addLineDraft(KEY_A, note());
    addLineDraft(KEY_A, note({ line: 12, lineText: "const c = 4;", body: "And this." }));
    addLineDraft(KEY_B, note({ body: "A note on another pull request." }));

    // Reading a different PR — and coming back — leaves both alone.
    expect(getLineDrafts(KEY_B)).toHaveLength(1);
    expect(getLineDrafts(KEY_A)).toHaveLength(2);
    expect(getLineDrafts(KEY_A).map((d) => d.body)).toEqual([
      "Prefer a named constant here.",
      "And this.",
    ]);
  });

  it("counts notes and files the way the footer says them", () => {
    addLineDraft(KEY_A, note());
    addLineDraft(KEY_A, note({ line: 12 }));
    addLineDraft(KEY_A, note({ path: "src/b.ts", line: 4 }));
    expect(draftCounts(getLineDrafts(KEY_A))).toEqual({
      notes: 3,
      files: 2,
      unanchored: 0,
    });
  });

  it("edits and deletes one note without touching the others", () => {
    const first = addLineDraft(KEY_A, note());
    const second = addLineDraft(KEY_A, note({ line: 12, body: "Second." }));

    updateLineDraft(KEY_A, first.id, { body: "Rewritten." });
    expect(getLineDrafts(KEY_A)[0].body).toBe("Rewritten.");
    expect(getLineDrafts(KEY_A)[1].body).toBe("Second.");

    removeLineDraft(KEY_A, second.id);
    expect(getLineDrafts(KEY_A)).toHaveLength(1);
  });

  it("discards only the PR you discarded", () => {
    addLineDraft(KEY_A, note());
    addLineDraft(KEY_B, note());
    clearLineDrafts(KEY_A);
    expect(getLineDrafts(KEY_A)).toHaveLength(0);
    expect(getLineDrafts(KEY_B)).toHaveLength(1);
  });

  it("hands back a new array each time so subscribers repaint", () => {
    const before = getLineDrafts(KEY_A);
    addLineDraft(KEY_A, note());
    expect(getLineDrafts(KEY_A)).not.toBe(before);
  });
});

describe("re-anchoring after a force-push", () => {
  it("moves what it is sure about and labels what it isn't", () => {
    const moving = addLineDraft(KEY_A, note());
    const doomed = addLineDraft(
      KEY_A,
      note({
        line: 12,
        lineText: "const c = 4;",
        contextBefore: "const b = 3;",
        contextAfter: "const d = 5;",
        body: "This line is about to be rewritten.",
      }),
    );

    const summary = reanchorLineDrafts(KEY_A, AFTER, NEW_OID);
    expect(summary).toEqual({ moved: 1, unanchored: 1, unchanged: 0 });

    const [a, b] = getLineDrafts(KEY_A);
    expect(a.id).toBe(moving.id);
    expect(a).toMatchObject({ status: "moved", line: 13, movedFrom: 11, headOidAtDraft: NEW_OID });
    expect(b.id).toBe(doomed.id);
    // Kept, with its words and its old anchor — nothing is deleted here.
    expect(b).toMatchObject({ status: "unanchored", line: 12, headOidAtDraft: OID });
    expect(b.body).toBe("This line is about to be rewritten.");
  });

  it("is a no-op while the head hasn't moved", () => {
    addLineDraft(KEY_A, note());
    expect(reanchorLineDrafts(KEY_A, BEFORE, OID)).toEqual({
      moved: 0,
      unanchored: 0,
      unchanged: 1,
    });
    expect(getLineDrafts(KEY_A)[0].status).toBe("pinned");
  });

  it("unanchors a range whose ends no longer bracket anything", () => {
    addLineDraft(
      KEY_A,
      note({
        startLine: 11,
        line: 12,
        startLineText: "const b = 3;",
        lineText: "const c = 4;",
        body: "Both of these.",
      }),
    );
    reanchorLineDrafts(KEY_A, AFTER, NEW_OID);
    // The end of the range was rewritten, so the range is gone even
    // though its first line still exists.
    expect(getLineDrafts(KEY_A)[0].status).toBe("unanchored");
  });

  it("moves a range whose ends both survive", () => {
    addLineDraft(
      KEY_A,
      note({
        startLine: 10,
        line: 11,
        startLineText: "const a = 1;",
        lineText: "const b = 3;",
        contextBefore: null,
        contextAfter: "const c = 4;",
      }),
    );
    reanchorLineDrafts(KEY_A, AFTER, NEW_OID);
    expect(getLineDrafts(KEY_A)[0]).toMatchObject({
      status: "moved",
      startLine: 10,
      line: 13,
    });
  });
});

describe("submitBlockedReason", () => {
  it("says nothing when every note is pinned to the current head", () => {
    addLineDraft(KEY_A, note());
    expect(submitBlockedReason(getLineDrafts(KEY_A), OID)).toBeNull();
  });

  it("blocks on an unanchored note and names the consequence", () => {
    addLineDraft(KEY_A, note({ lineText: "gone forever" }));
    reanchorLineDrafts(KEY_A, AFTER, NEW_OID);
    const reason = submitBlockedReason(getLineDrafts(KEY_A), NEW_OID);
    expect(reason).toContain("no longer match a line");
    // Rule 5: the reason says what it costs, not just that it's blocked.
    expect(reason).toContain("all of them would fail");
  });

  it("blocks a note still pinned to a superseded head", () => {
    addLineDraft(KEY_A, note());
    const reason = submitBlockedReason(getLineDrafts(KEY_A), NEW_OID);
    expect(reason).toContain("older version of this branch");
    expect(reason).toContain("Re-anchor");
  });
});

describe("diff snapshots", () => {
  it("keeps the diff a note was written against", () => {
    putDiffSnapshot(KEY_A, OID, BEFORE);
    putDiffSnapshot(KEY_A, NEW_OID, AFTER);
    expect(getDiffSnapshot(KEY_A, OID)).toBe(BEFORE);
    expect(getDiffSnapshot(KEY_A, NEW_OID)).toBe(AFTER);
    expect(getDiffSnapshot(KEY_A, "never-seen")).toBeNull();
  });

  it("keeps a bounded history rather than every diff of the afternoon", () => {
    putDiffSnapshot(KEY_A, "one", "1");
    putDiffSnapshot(KEY_A, "two", "2");
    putDiffSnapshot(KEY_A, "three", "3");
    putDiffSnapshot(KEY_A, "four", "4");
    expect(getDiffSnapshot(KEY_A, "one")).toBeNull();
    expect(getDiffSnapshot(KEY_A, "four")).toBe("4");
  });
});
