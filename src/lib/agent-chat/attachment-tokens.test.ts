import { describe, expect, it } from "vitest";

import {
  activeAttachments,
  parseFileTokens,
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
