import { describe, expect, it } from "vitest";

import {
  buildAttachmentBlock,
  buildFileResolvedContent,
} from "./attachment-block";
import { applyAllPrefixes } from "./mode-prefix";
import type { Attachment } from "@/stores/agent-chat-store";
import type { FileAttachmentInfo } from "@/tauri/types";

function makeFileAttachment(
  overrides: Partial<Attachment> = {},
): Attachment {
  return {
    id: "att-1",
    kind: "file",
    ref: "/abs/path/to/Composer.tsx",
    metadata: { label: "Composer.tsx", lineCount: 421 },
    resolvedContent: "```tsx\n// composer body\n```",
    ...overrides,
  };
}

function makeInfo(overrides: Partial<FileAttachmentInfo> = {}): FileAttachmentInfo {
  return {
    absolutePath: "/abs/Composer.tsx",
    relativePath: "src/components/chat/Composer.tsx",
    lineCount: 3,
    bytes: 32,
    language: "tsx",
    isText: true,
    content: "// hello\nexport function f() {}\n",
    truncated: false,
    outline: null,
    ...overrides,
  };
}

describe("buildAttachmentBlock", () => {
  it("returns null when the list is empty", () => {
    expect(buildAttachmentBlock([])).toBeNull();
  });

  it("returns null when no attachments have resolved content", () => {
    const att = makeFileAttachment({ resolvedContent: undefined });
    expect(buildAttachmentBlock([att])).toBeNull();
  });

  it("skips attachments whose resolvedContent is the empty string", () => {
    const att = makeFileAttachment({ resolvedContent: "" });
    expect(buildAttachmentBlock([att])).toBeNull();
  });

  it("wraps a single resolved file in the sentinel-fenced block", () => {
    const att = makeFileAttachment();
    const block = buildAttachmentBlock([att]);
    expect(block).not.toBeNull();
    expect(block!).toContain("=== Attached context ===");
    expect(block!).toContain("=== End context ===");
    expect(block!).toContain("## File: Composer.tsx");
    expect(block!).toContain("Full path: /abs/path/to/Composer.tsx");
    expect(block!).toContain("Lines: 421");
    expect(block!).toContain("// composer body");
  });

  it("includes every resolved file in insertion order", () => {
    const a = makeFileAttachment({
      id: "a",
      ref: "/a.ts",
      metadata: { label: "a.ts" },
      resolvedContent: "```ts\nA\n```",
    });
    const b = makeFileAttachment({
      id: "b",
      ref: "/b.ts",
      metadata: { label: "b.ts" },
      resolvedContent: "```ts\nB\n```",
    });
    const block = buildAttachmentBlock([a, b]);
    expect(block).not.toBeNull();
    const aIdx = block!.indexOf("## File: a.ts");
    const bIdx = block!.indexOf("## File: b.ts");
    expect(aIdx).toBeGreaterThanOrEqual(0);
    expect(bIdx).toBeGreaterThan(aIdx);
  });

  it("excludes images even when they have content somehow", () => {
    const img: Attachment = {
      id: "img",
      kind: "image",
      ref: "image:1",
      metadata: { label: "screenshot.png" },
      resolvedContent: "should be ignored",
    };
    expect(buildAttachmentBlock([img])).toBeNull();
  });

  it("omits the Lines header when lineCount is undefined", () => {
    const att = makeFileAttachment({
      metadata: { label: "Composer.tsx" },
    });
    const block = buildAttachmentBlock([att]);
    expect(block!).not.toContain("Lines: ");
    expect(block!).toContain("## File: Composer.tsx");
  });
});

describe("buildFileResolvedContent", () => {
  it("renders full content fenced by language for small text files", () => {
    const out = buildFileResolvedContent(
      makeInfo({ content: "// alpha\n// beta\n", language: "ts" }),
    );
    expect(out).toContain("```ts");
    expect(out).toContain("// alpha");
    expect(out).toContain("// beta");
    // Truncated-only artifacts MUST NOT appear in the full path.
    expect(out).not.toContain("First 50 of");
    expect(out).not.toContain("Outline (");
    expect(out).not.toContain("Use the Read tool");
  });

  it("renders the truncated preview + outline + Read-tool hint for large files", () => {
    const lines = Array.from({ length: 200 }, (_, i) => `// line ${i + 1}`);
    const info = makeInfo({
      content: lines.join("\n"),
      lineCount: 1800,
      truncated: true,
      outline: [
        { kind: "function", name: "alpha", line: 12 },
        { kind: "class", name: "Beta", line: 80 },
      ],
    });
    const out = buildFileResolvedContent(info);
    expect(out).toContain("First 50 of 1800 lines:");
    expect(out).toContain("```tsx");
    // Only the first 50 preview lines should appear in the body.
    expect(out).toContain("// line 1\n");
    expect(out).toContain("// line 50");
    expect(out).not.toContain("// line 51");
    expect(out).toContain("Outline (2 declarations):");
    expect(out).toContain("- function alpha (line 12)");
    expect(out).toContain("- class Beta (line 80)");
    expect(out).toContain('Use the Read tool with path "/abs/Composer.tsx"');
  });

  it("renders a binary-file marker when isText is false", () => {
    const info = makeInfo({ isText: false, content: "" });
    expect(buildFileResolvedContent(info)).toContain("[binary file");
  });

  it("omits the outline section when truncated but outline is empty", () => {
    const info = makeInfo({
      content: "x",
      truncated: true,
      outline: [],
    });
    const out = buildFileResolvedContent(info);
    expect(out).not.toContain("Outline (");
    expect(out).toContain("First 50 of");
  });

  it("uses an empty language fence when language is null", () => {
    const out = buildFileResolvedContent(
      makeInfo({ language: null, content: "raw\n" }),
    );
    // Empty fence = "```\n" (no language hint).
    expect(out).toMatch(/```\n/);
  });
});

describe("Stage 2 sample — full applyAllPrefixes pipeline with file attachment", () => {
  // Smoke-test the full pipeline shape so Stage 2 deliverable can
  // quote what the agent actually receives without hand-rolling the
  // composition. Run with `--nocapture` style: this test prints to
  // stderr when DEBUG_STAGE2_SAMPLE=1 is set, otherwise it just
  // asserts the structure invariants.
  it("renders a recognizable wrapped prompt for a small file pick", () => {
    const info: FileAttachmentInfo = {
      absolutePath: "/repo/src/components/chat/Composer.tsx",
      relativePath: "src/components/chat/Composer.tsx",
      lineCount: 421,
      bytes: 16384,
      language: "tsx",
      isText: true,
      content:
        "import { File as FileIcon } from \"lucide-react\";\n" +
        "import { useState } from \"react\";\n" +
        "// (truncated in fixture)\n",
      truncated: false,
      outline: null,
    };
    const att: Attachment = {
      id: "att-stage2-sample",
      kind: "file",
      ref: info.absolutePath,
      metadata: { label: "Composer.tsx", lineCount: info.lineCount },
      resolvedContent: buildFileResolvedContent(info),
    };
    const block = buildAttachmentBlock([att]);
    const wrapped = applyAllPrefixes(
      "explain this file's main responsibilities",
      "default",
      "ultrathink",
      null,
      block,
    );
    // Hard-asserted structural invariants — these are the contract
    // every Stage 2 send must satisfy.
    expect(wrapped).toContain("Ultrathink");
    expect(wrapped).toContain("=== Attached context ===");
    expect(wrapped).toContain("## File: Composer.tsx");
    expect(wrapped).toContain("Full path: /repo/src/components/chat/Composer.tsx");
    expect(wrapped).toContain("Lines: 421");
    expect(wrapped).toContain("```tsx");
    expect(wrapped).toContain("=== End context ===");
    expect(wrapped).toContain("explain this file's main responsibilities");
  });
});
