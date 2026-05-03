import { describe, expect, it } from "vitest";

import {
  activeAttachments,
  parseFileTokens,
  parseIssueTokens,
  parsePrTokens,
  segmentDraftHighlight,
} from "./attachment-tokens";
import type { Attachment } from "@/stores/agent-chat-store";
import type { Skill } from "@/tauri/commands";

function makeFileAttachment(overrides: Partial<Attachment> = {}): Attachment {
  return {
    id: "att-1",
    kind: "file",
    ref: "/repo/src/components/chat/Composer.tsx",
    metadata: { label: "Composer.tsx", lineCount: 421 },
    resolvedContent: "```tsx\n// composer\n```",
    ...overrides,
  };
}

function makeSkill(name: string): Skill {
  return {
    id: `skill-${name}`,
    name,
    description: `${name} skill`,
    provider: "claude",
    scope: "user",
    skillDir: `/skills/${name}`,
    filePath: `/skills/${name}/SKILL.md`,
    body: `body of ${name}`,
    rawFrontmatter: {},
    bundledFiles: [],
    compatibility: "compatible",
    compatibilitySignals: [],
    symlinked: false,
    pluginSlug: null,
  };
}

describe("parseFileTokens", () => {
  it("returns no matches when text is empty", () => {
    expect(parseFileTokens("", [makeFileAttachment()])).toEqual([]);
  });

  it("returns no matches when no attachments are staged", () => {
    expect(parseFileTokens("@Composer.tsx", [])).toEqual([]);
  });

  it("matches @<basename> when a staged attachment label matches", () => {
    const att = makeFileAttachment();
    const matches = parseFileTokens("look at @Composer.tsx now", [att]);
    expect(matches).toHaveLength(1);
    expect(matches[0]).toMatchObject({
      basename: "Composer.tsx",
      token: "@Composer.tsx",
      attachment: att,
    });
    expect(matches[0]?.start).toBe(8);
    expect(matches[0]?.end).toBe(8 + "@Composer.tsx".length);
  });

  it("requires whitespace or start-of-text before @ (rejects in-word)", () => {
    const att = makeFileAttachment();
    expect(parseFileTokens("a@Composer.tsx", [att])).toEqual([]);
  });

  it("matches at start of text", () => {
    const att = makeFileAttachment();
    const matches = parseFileTokens("@Composer.tsx and more", [att]);
    expect(matches).toHaveLength(1);
    expect(matches[0]?.start).toBe(0);
  });

  it("matches multiple distinct tokens", () => {
    const a = makeFileAttachment({ id: "a", metadata: { label: "A.ts" } });
    const b = makeFileAttachment({ id: "b", metadata: { label: "B.ts" } });
    const matches = parseFileTokens("first @A.ts then @B.ts done", [a, b]);
    expect(matches.map((m) => m.basename)).toEqual(["A.ts", "B.ts"]);
  });

  it("ignores tokens whose basename has no staged attachment", () => {
    const att = makeFileAttachment();
    const matches = parseFileTokens("@Composer.tsx and @Unknown.ts", [att]);
    expect(matches).toHaveLength(1);
    expect(matches[0]?.basename).toBe("Composer.tsx");
  });

  it("ignores image attachments (image kind has no inline token)", () => {
    const img: Attachment = {
      id: "img",
      kind: "image",
      ref: "image:1",
      metadata: { label: "screenshot.png" },
    };
    expect(parseFileTokens("@screenshot.png here", [img])).toEqual([]);
  });

  it("matches folder kind too (folders share the inline-token model with files)", () => {
    const folder: Attachment = {
      id: "f",
      kind: "folder",
      ref: "/repo/src",
      metadata: { label: "src" },
    };
    const matches = parseFileTokens("look at @src folder", [folder]);
    expect(matches).toHaveLength(1);
    expect(matches[0]?.attachment.kind).toBe("folder");
  });

  it("requires a non-name terminator after the basename (rejects partial)", () => {
    const att = makeFileAttachment();
    // Token is `@Composer.tsxxxx` — name continues, doesn't match `Composer.tsx`.
    expect(parseFileTokens("@Composer.tsxxxx", [att])).toEqual([]);
  });

  it("is stable across multiple calls (regex global flag handled)", () => {
    const att = makeFileAttachment();
    const text = "@Composer.tsx";
    const a = parseFileTokens(text, [att]);
    const b = parseFileTokens(text, [att]);
    expect(a).toEqual(b);
  });
});

describe("activeAttachments", () => {
  it("filters out attachments whose token is not present in text", () => {
    const a = makeFileAttachment({ id: "a", metadata: { label: "A.ts" } });
    const b = makeFileAttachment({ id: "b", metadata: { label: "B.ts" } });
    expect(activeAttachments("only @A.ts", [a, b])).toEqual([a]);
    expect(activeAttachments("nothing here", [a, b])).toEqual([]);
  });

  it("preserves source-position order for tokens", () => {
    const a = makeFileAttachment({ id: "a", metadata: { label: "A.ts" } });
    const b = makeFileAttachment({ id: "b", metadata: { label: "B.ts" } });
    expect(activeAttachments("@B.ts then @A.ts", [a, b])).toEqual([b, a]);
  });

  it("dedupes when the same attachment is mentioned twice", () => {
    const att = makeFileAttachment();
    const out = activeAttachments(
      "@Composer.tsx and again @Composer.tsx",
      [att],
    );
    expect(out).toHaveLength(1);
    expect(out[0]).toBe(att);
  });

  it("keeps image attachments unconditionally (out-of-text)", () => {
    const file = makeFileAttachment({ id: "f", metadata: { label: "f.ts" } });
    const img: Attachment = {
      id: "i",
      kind: "image",
      ref: "image:1",
      metadata: { label: "shot.png" },
    };
    // No text token for the file → file dropped, image kept.
    expect(activeAttachments("nothing", [file, img])).toEqual([img]);
  });

  it("returns the input unchanged when the list is already empty", () => {
    expect(activeAttachments("anything", [])).toEqual([]);
  });
});

describe("segmentDraftHighlight", () => {
  it("returns an empty array for empty text", () => {
    expect(segmentDraftHighlight("", [], [])).toEqual([]);
  });

  it("returns a single plain segment when nothing matches", () => {
    expect(segmentDraftHighlight("hello world", [], [])).toEqual([
      { kind: "plain", text: "hello world" },
    ]);
  });

  it("emits skill segments for `/<skill>` tokens", () => {
    const skill = makeSkill("plan");
    const out = segmentDraftHighlight("/plan now", [skill], []);
    expect(out).toEqual([
      { kind: "skill", text: "/plan", name: "plan" },
      { kind: "plain", text: " now" },
    ]);
  });

  it("emits attachment segments for `@<basename>` tokens", () => {
    const att = makeFileAttachment();
    const out = segmentDraftHighlight("look @Composer.tsx", [], [att]);
    expect(out).toEqual([
      { kind: "plain", text: "look " },
      {
        kind: "attachment",
        text: "@Composer.tsx",
        basename: "Composer.tsx",
        isLoading: false,
        hasError: false,
      },
    ]);
  });

  it("interleaves skill + attachment segments by source position", () => {
    const skill = makeSkill("plan");
    const att = makeFileAttachment();
    const out = segmentDraftHighlight(
      "/plan look @Composer.tsx",
      [skill],
      [att],
    );
    expect(out.map((s) => s.kind)).toEqual([
      "skill",
      "plain",
      "attachment",
    ]);
  });

  it("propagates isLoading=true to the segment", () => {
    const att = makeFileAttachment({
      metadata: { label: "Composer.tsx", isLoading: true },
    });
    const out = segmentDraftHighlight("@Composer.tsx", [], [att]);
    const chip = out.find((s) => s.kind === "attachment");
    expect(chip?.kind).toBe("attachment");
    if (chip?.kind === "attachment") {
      expect(chip.isLoading).toBe(true);
      expect(chip.hasError).toBe(false);
    }
  });

  it("propagates hasError=true when metadata.error is set", () => {
    const att = makeFileAttachment({
      metadata: { label: "Composer.tsx", error: "permission denied" },
    });
    const out = segmentDraftHighlight("@Composer.tsx", [], [att]);
    const chip = out.find((s) => s.kind === "attachment");
    if (chip?.kind === "attachment") {
      expect(chip.hasError).toBe(true);
    } else {
      throw new Error("expected an attachment segment");
    }
  });

  it("preserves text byte-for-byte across plain runs (cursor alignment)", () => {
    const att = makeFileAttachment();
    const text = "  hello\t@Composer.tsx\nworld  ";
    const out = segmentDraftHighlight(text, [], [att]);
    const reassembled = out.map((s) => s.text).join("");
    expect(reassembled).toBe(text);
  });
});

function makeIssueAttachment(
  overrides: Partial<Attachment> = {},
): Attachment {
  return {
    id: "att-issue-21",
    kind: "issue",
    ref: "#21",
    metadata: {
      label: "#21 Login redirect bug",
      state: "open",
    },
    resolvedContent: undefined,
    ...overrides,
  };
}

describe("parseIssueTokens", () => {
  it("matches @#<n> against a staged issue attachment by ref", () => {
    const att = makeIssueAttachment();
    const matches = parseIssueTokens("triage @#21 today", [att]);
    expect(matches).toHaveLength(1);
    expect(matches[0]).toMatchObject({
      basename: "#21",
      token: "@#21",
      attachment: att,
    });
  });

  it("ignores @#<n> when no staged issue matches the number", () => {
    const att = makeIssueAttachment({ ref: "#21" });
    expect(parseIssueTokens("look at @#42", [att])).toHaveLength(0);
  });

  it("ignores file-style @<name> tokens — those go through parseFileTokens", () => {
    const issueAtt = makeIssueAttachment();
    expect(parseIssueTokens("@Composer.tsx", [issueAtt])).toHaveLength(0);
  });

  it("anchors at start-of-text or whitespace, not inside a word", () => {
    const att = makeIssueAttachment();
    // `a@#21` should NOT match — `@` is mid-word.
    expect(parseIssueTokens("a@#21", [att])).toHaveLength(0);
    expect(parseIssueTokens("@#21", [att])).toHaveLength(1);
    expect(parseIssueTokens(" @#21", [att])).toHaveLength(1);
    expect(parseIssueTokens("\n@#21", [att])).toHaveLength(1);
  });

  it("stops the match at non-digit characters (no comma sweep)", () => {
    const att = makeIssueAttachment();
    const matches = parseIssueTokens("see @#21, please", [att]);
    expect(matches).toHaveLength(1);
    expect(matches[0]?.token).toBe("@#21");
  });

  it("returns empty list when no issue attachments are staged", () => {
    const fileAtt = makeFileAttachment();
    expect(parseIssueTokens("@#21", [fileAtt])).toHaveLength(0);
  });
});

describe("activeAttachments — issue tokens", () => {
  it("includes a staged issue when its @#<n> token appears in text", () => {
    const att = makeIssueAttachment();
    const out = activeAttachments("close @#21 first", [att]);
    expect(out).toEqual([att]);
  });

  it("excludes a staged issue whose @#<n> token was deleted", () => {
    const att = makeIssueAttachment();
    expect(activeAttachments("nothing here", [att])).toEqual([]);
  });

  it("interleaves file + issue tokens by source position", () => {
    const issue = makeIssueAttachment();
    const file = makeFileAttachment();
    const out = activeAttachments(
      "see @#21 then @Composer.tsx",
      [file, issue],
    );
    // `@#21` appears first → issue ordered before file even though
    // the staged-attachments array had file first.
    expect(out).toEqual([issue, file]);
  });
});

describe("segmentDraftHighlight — issue tokens", () => {
  it("emits an `issue-attachment` segment with state from metadata", () => {
    const att = makeIssueAttachment({
      metadata: { label: "#21 …", state: "open" },
    });
    const out = segmentDraftHighlight("see @#21 now", [], [att]);
    const seg = out.find((s) => s.kind === "issue-attachment");
    expect(seg).toBeDefined();
    if (seg?.kind === "issue-attachment") {
      expect(seg.text).toBe("@#21");
      expect(seg.ref).toBe("#21");
      expect(seg.state).toBe("open");
      expect(seg.isLoading).toBe(false);
      expect(seg.hasError).toBe(false);
    }
  });

  it("flags state=closed when the staged attachment was closed", () => {
    const att = makeIssueAttachment({
      metadata: { label: "#21 …", state: "closed" },
    });
    const out = segmentDraftHighlight("see @#21 now", [], [att]);
    const seg = out.find((s) => s.kind === "issue-attachment");
    if (seg?.kind === "issue-attachment") {
      expect(seg.state).toBe("closed");
    }
  });

  it("forwards isLoading + hasError flags from the attachment metadata", () => {
    const loading = makeIssueAttachment({
      id: "att-loading",
      ref: "#21",
      metadata: { label: "#21", state: "open", isLoading: true },
    });
    const errored = makeIssueAttachment({
      id: "att-error",
      ref: "#22",
      metadata: { label: "#22", state: "open", error: "rate-limited" },
    });
    const out = segmentDraftHighlight("@#21 @#22", [], [loading, errored]);
    const segs = out.filter(
      (s) => s.kind === "issue-attachment",
    ) as Array<Extract<typeof out[number], { kind: "issue-attachment" }>>;
    expect(segs).toHaveLength(2);
    expect(segs[0]?.isLoading).toBe(true);
    expect(segs[0]?.hasError).toBe(false);
    expect(segs[1]?.isLoading).toBe(false);
    expect(segs[1]?.hasError).toBe(true);
  });

  it("preserves byte-for-byte text across plain runs with mixed tokens", () => {
    const file = makeFileAttachment();
    const issue = makeIssueAttachment();
    const text = "fix @Composer.tsx for @#21 now";
    const out = segmentDraftHighlight(text, [], [file, issue]);
    expect(out.map((s) => s.text).join("")).toBe(text);
  });
});

function makePrAttachment(overrides: Partial<Attachment> = {}): Attachment {
  return {
    id: "att-pr-7",
    kind: "pr",
    ref: "!7",
    metadata: {
      label: "#7 Refactor auth",
      state: "open",
    },
    resolvedContent: undefined,
    ...overrides,
  };
}

describe("parsePrTokens", () => {
  it("matches @!<n> against a staged PR by ref", () => {
    const att = makePrAttachment();
    const matches = parsePrTokens("review @!7 today", [att]);
    expect(matches).toHaveLength(1);
    expect(matches[0]).toMatchObject({
      basename: "!7",
      token: "@!7",
      attachment: att,
    });
  });

  it("ignores @!<n> for an unstaged PR number", () => {
    const att = makePrAttachment({ ref: "!7" });
    expect(parsePrTokens("@!42", [att])).toHaveLength(0);
  });

  it("ignores @#<n> issue tokens — those go through parseIssueTokens", () => {
    const pr = makePrAttachment();
    expect(parsePrTokens("@#7", [pr])).toHaveLength(0);
  });

  it("anchors at start-of-text or whitespace", () => {
    const att = makePrAttachment();
    expect(parsePrTokens("a@!7", [att])).toHaveLength(0);
    expect(parsePrTokens("@!7", [att])).toHaveLength(1);
    expect(parsePrTokens(" @!7", [att])).toHaveLength(1);
    expect(parsePrTokens("\n@!7", [att])).toHaveLength(1);
  });

  it("stops the match at non-digit characters (no comma sweep)", () => {
    const att = makePrAttachment();
    const matches = parsePrTokens("see @!7, please", [att]);
    expect(matches).toHaveLength(1);
    expect(matches[0]?.token).toBe("@!7");
  });
});

describe("activeAttachments — PR tokens", () => {
  it("includes a staged PR when its @!<n> token appears", () => {
    const att = makePrAttachment();
    expect(activeAttachments("merge @!7 first", [att])).toEqual([att]);
  });

  it("excludes a staged PR whose @!<n> token was deleted", () => {
    const att = makePrAttachment();
    expect(activeAttachments("nothing here", [att])).toEqual([]);
  });

  it("interleaves file + issue + PR tokens by source position", () => {
    const file = makeFileAttachment();
    const issue = makeIssueAttachment();
    const pr = makePrAttachment();
    // Tokens appear in source order: pr → file → issue.
    const out = activeAttachments(
      "see @!7 then @Composer.tsx and @#21",
      [file, issue, pr],
    );
    expect(out).toEqual([pr, file, issue]);
  });
});

describe("segmentDraftHighlight — PR tokens", () => {
  it("emits a `pr-attachment` segment with state from metadata", () => {
    const att = makePrAttachment({
      metadata: { label: "#7 …", state: "open" },
    });
    const out = segmentDraftHighlight("review @!7 now", [], [att]);
    const seg = out.find((s) => s.kind === "pr-attachment");
    expect(seg).toBeDefined();
    if (seg?.kind === "pr-attachment") {
      expect(seg.text).toBe("@!7");
      expect(seg.ref).toBe("!7");
      expect(seg.state).toBe("open");
      expect(seg.isLoading).toBe(false);
      expect(seg.hasError).toBe(false);
    }
  });

  it("forwards merged / draft / closed states", () => {
    for (const state of ["merged", "draft", "closed"] as const) {
      const att = makePrAttachment({
        id: `att-${state}`,
        ref: "!7",
        metadata: { label: "#7", state },
      });
      const out = segmentDraftHighlight("@!7", [], [att]);
      const seg = out.find((s) => s.kind === "pr-attachment");
      if (seg?.kind === "pr-attachment") {
        expect(seg.state).toBe(state);
      } else {
        throw new Error(`expected pr-attachment seg for state=${state}`);
      }
    }
  });

  it("defaults unknown state to 'open'", () => {
    // Edge: an addStagedAttachment without state set still resolves
    // to a sensible default rather than crashing the renderer.
    const att = makePrAttachment({
      metadata: { label: "#7" },
    });
    const out = segmentDraftHighlight("@!7", [], [att]);
    const seg = out.find((s) => s.kind === "pr-attachment");
    if (seg?.kind === "pr-attachment") {
      expect(seg.state).toBe("open");
    }
  });

  it("preserves byte-for-byte text with file + issue + pr tokens", () => {
    const file = makeFileAttachment();
    const issue = makeIssueAttachment();
    const pr = makePrAttachment();
    const text = "fix @Composer.tsx + @#21 + @!7";
    const out = segmentDraftHighlight(text, [], [file, issue, pr]);
    expect(out.map((s) => s.text).join("")).toBe(text);
  });
});
