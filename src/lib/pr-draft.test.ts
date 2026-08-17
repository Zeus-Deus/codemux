import { describe, expect, it } from "vitest";

import {
  draftBody,
  draftTitle,
  humanizeBranch,
  joinPath,
  verificationFrom,
} from "./pr-draft";
import type { CommitSummary } from "@/tauri/types";

const commit = (subject: string, body = ""): CommitSummary => ({
  short_hash: subject.slice(0, 7),
  subject,
  body,
});

describe("humanizeBranch", () => {
  it("drops the type segment rather than title-casing it", () => {
    expect(humanizeBranch("feat/drop-ports-guidance")).toBe("Drop ports guidance");
    expect(humanizeBranch("fix_windows_shutdown")).toBe("Windows shutdown");
  });

  it("is empty for a branch that isn't there", () => {
    expect(humanizeBranch(null)).toBe("");
  });
});

describe("draftTitle", () => {
  it("uses the subject verbatim for a single commit", () => {
    const result = draftTitle(
      [commit("docs: drop the ports section from AGENTS.md")],
      "docs/drop-ports",
    );
    expect(result).toEqual({
      value: "docs: drop the ports section from AGENTS.md",
      source: "commits",
    });
  });

  it("keeps the shared prefix and the shared words across commits", () => {
    const result = draftTitle(
      [
        commit("feat(chat): stream replies without buffering"),
        commit("feat(chat): stream replies through one channel"),
      ],
      "feature/75-chat-channel",
    );
    expect(result).toEqual({ value: "feat(chat): stream replies", source: "commits" });
  });

  it("falls back to the branch as the summary when only the prefix agrees", () => {
    const result = draftTitle(
      [
        commit("feat(chat): stream replies"),
        commit("feat(chat): drop the old socket"),
      ],
      "feature/chat-channel",
    );
    expect(result).toEqual({ value: "feat(chat): chat channel", source: "commits" });
  });

  it("drops the scope when the commits disagree about it", () => {
    const result = draftTitle(
      [commit("fix(chat): one"), commit("fix(review): two")],
      "fix/two-things",
    );
    expect(result.value).toBe("fix: two things");
  });

  it("does not claim the commits wrote it when they agree on nothing", () => {
    const result = draftTitle(
      [commit("wip"), commit("more wip"), commit("fix the thing")],
      "chore/port-detection",
    );
    // The field is still filled in — but from the branch, and the note
    // that says otherwise must not render.
    expect(result).toEqual({ value: "Port detection", source: "branch" });
  });

  it("uses the branch when there are no commits at all", () => {
    expect(draftTitle([], "feat/new-thing")).toEqual({
      value: "New thing",
      source: "branch",
    });
  });
});

describe("draftBody", () => {
  it("uses a single commit's body, without its trailers", () => {
    const result = draftBody([
      commit(
        "fix: stop the flicker",
        "The pane remounted on every poll.\n\nCo-Authored-By: someone <a@b.c>",
      ),
    ]);
    expect(result.value).toBe("The pane remounted on every poll.");
    expect(result.source).toBe("commits");
  });

  it("says nothing when a single commit had nothing to add", () => {
    expect(draftBody([commit("fix: stop the flicker")])).toEqual({
      value: "",
      source: "none",
    });
  });

  it("lists several commits oldest first", () => {
    const result = draftBody([commit("feat: third"), commit("feat: second"), commit("feat: first")]);
    expect(result.value).toBe("- feat: first\n- feat: second\n- feat: third");
  });

  it("carries a verification line a commit already stated", () => {
    const result = draftBody([
      commit("feat: b"),
      commit("feat: a", "Verification · cargo test and web checks run clean"),
    ]);
    expect(result.value).toContain(
      "Verification · cargo test and web checks run clean",
    );
  });

  it("never invents one", () => {
    const result = draftBody([commit("feat: b"), commit("feat: a", "Some prose.")]);
    expect(result.value).not.toContain("Verification");
    expect(verificationFrom([commit("feat: a", "Some prose.")])).toBeNull();
  });
});

describe("joinPath", () => {
  it("joins with the separator the root already uses", () => {
    expect(joinPath("/home/dev/repo", ".github/PULL_REQUEST_TEMPLATE.md")).toBe(
      "/home/dev/repo/.github/PULL_REQUEST_TEMPLATE.md",
    );
    expect(joinPath("C:\\dev\\repo", ".github/x.md")).toBe("C:\\dev\\repo\\.github\\x.md");
  });
});
