import { describe, expect, it } from "vitest";

import {
  buildAttachmentBlock,
  buildFileResolvedContent,
  buildFolderResolvedContent,
  buildImageDisplaySources,
  buildImageRefs,
  buildIssueResolvedContent,
  buildPrResolvedContent,
  imageAttachmentIds,
  unstagedImageAttachments,
} from "./attachment-block";
import { applyAllPrefixes } from "./mode-prefix";
import type { Attachment } from "@/stores/agent-chat-store";
import type {
  FileAttachmentInfo,
  GitHubIssue,
  PullRequestInfo,
} from "@/tauri/types";

function makePrDetail(overrides: Partial<PullRequestInfo> = {}): PullRequestInfo {
  return {
    number: 42,
    url: "https://github.com/u/r/pull/42",
    state: "OPEN",
    title: "Add dark mode",
    head_branch: "feat/dark",
    base_branch: "main",
    is_draft: false,
    mergeable: "MERGEABLE",
    additions: 100,
    deletions: 5,
    review_decision: "APPROVED",
    checks_passing: null,
    updated_at: null,
    body: "PR body here",
    comments: [],
    totalComments: 0,
    author: "alice",
    ...overrides,
  };
}

function makeIssueDetail(overrides: Partial<GitHubIssue> = {}): GitHubIssue {
  return {
    number: 42,
    title: "Add dark mode",
    state: "Open",
    labels: ["enhancement"],
    assignees: ["zeus"],
    url: "https://github.com/u/r/issues/42",
    body: "Body line 1\nBody line 2",
    comments: [],
    totalComments: 0,
    updatedAt: null,
    ...overrides,
  };
}

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

describe("buildFolderResolvedContent (Step 8 Stage 3)", () => {
  it("renders the tree fenced + a Read/Grep tool hint with the absolute path", () => {
    const out = buildFolderResolvedContent({
      absolutePath: "/repo/src/components/chat",
      relativePath: "src/components/chat",
      tree: "chat\n├── Composer.tsx\n└── pickers/",
      itemCount: 4,
    });
    expect(out).toContain("Tree (depth-bounded, 4 items):");
    expect(out).toContain("```");
    expect(out).toContain("├── Composer.tsx");
    expect(out).toContain("└── pickers/");
    expect(out).toContain('Use the Read or Grep tool with path "/repo/src/components/chat"');
  });

  it("uses singular form when itemCount is 1", () => {
    const out = buildFolderResolvedContent({
      absolutePath: "/repo/loner",
      relativePath: "loner",
      tree: "loner\n└── only.txt",
      itemCount: 1,
    });
    expect(out).toContain("1 item):");
    expect(out).not.toContain("1 items");
  });
});

describe("buildAttachmentBlock with folder kind (Step 8 Stage 3)", () => {
  function makeFolderAtt(overrides: Partial<Attachment> = {}): Attachment {
    return {
      id: "folder-1",
      kind: "folder",
      ref: "/repo/src/components/chat",
      metadata: { label: "chat" },
      resolvedContent: "Tree (depth-bounded, 3 items):\n```\nchat\n├── A.tsx\n└── B.tsx\n```\nUse the Read or Grep tool with path \"/repo/src/components/chat\" to explore further.",
      ...overrides,
    };
  }

  it("wraps a folder attachment with the ## Folder: header + path", () => {
    const block = buildAttachmentBlock([makeFolderAtt()]);
    expect(block).not.toBeNull();
    expect(block!).toContain("## Folder: chat");
    expect(block!).toContain("Path: /repo/src/components/chat");
    expect(block!).toContain("Tree (depth-bounded");
    expect(block!).toContain("=== Attached context ===");
  });

  it("interleaves files + folders in source order", () => {
    const file = makeFileAttachment({
      id: "f",
      ref: "/abs/Composer.tsx",
      metadata: { label: "Composer.tsx", lineCount: 100 },
      resolvedContent: "```tsx\nfile body\n```",
    });
    const folder = makeFolderAtt({ id: "fo" });
    const block = buildAttachmentBlock([file, folder]);
    const fileIdx = block!.indexOf("## File: Composer.tsx");
    const folderIdx = block!.indexOf("## Folder: chat");
    expect(fileIdx).toBeGreaterThanOrEqual(0);
    expect(folderIdx).toBeGreaterThan(fileIdx);
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

describe("buildIssueResolvedContent", () => {
  it("renders state, url, body and skips empty comment section", () => {
    const out = buildIssueResolvedContent(makeIssueDetail());
    expect(out).toContain("State: Open");
    expect(out).toContain("URL: https://github.com/u/r/issues/42");
    expect(out).toContain("### Body");
    expect(out).toContain("Body line 1");
    expect(out).not.toContain("### Comments");
  });

  it("falls back to '(no body)' when body is empty", () => {
    const out = buildIssueResolvedContent(makeIssueDetail({ body: null }));
    expect(out).toContain("(no body)");
    const out2 = buildIssueResolvedContent(makeIssueDetail({ body: "" }));
    expect(out2).toContain("(no body)");
  });

  it("renders comments section with author / timestamp / body", () => {
    const out = buildIssueResolvedContent(
      makeIssueDetail({
        comments: [
          {
            author: "alice",
            body: "agreed!",
            createdAt: "2026-01-02T00:00:00Z",
          },
        ],
        totalComments: 1,
      }),
    );
    expect(out).toContain(
      "### Comments (1 total, showing first 1)",
    );
    expect(out).toContain("**alice** (2026-01-02T00:00:00Z):");
    expect(out).toContain("agreed!");
    // Only one comment shown total — no truncation footer.
    expect(out).not.toContain("more comment");
  });

  it("includes a truncation footer when totalComments exceeds shown", () => {
    const comments = Array.from({ length: 20 }, (_, i) => ({
      author: `u${i}`,
      body: `c${i}`,
      createdAt: "2026-01-01T00:00:00Z",
    }));
    const out = buildIssueResolvedContent(
      makeIssueDetail({ comments, totalComments: 132 }),
    );
    expect(out).toContain("(132 total, showing first 20)");
    expect(out).toContain(
      "112 more comments not shown. Use `gh issue view 42 --comments`",
    );
  });

  it("singular phrasing when exactly one comment is hidden", () => {
    const comments = Array.from({ length: 20 }, (_, i) => ({
      author: `u${i}`,
      body: `c${i}`,
      createdAt: "2026-01-01T00:00:00Z",
    }));
    const out = buildIssueResolvedContent(
      makeIssueDetail({ comments, totalComments: 21 }),
    );
    expect(out).toContain("1 more comment not shown");
  });

  it("includes labels and assignees in the header when present", () => {
    const out = buildIssueResolvedContent(makeIssueDetail());
    expect(out).toContain("Labels: enhancement");
    expect(out).toContain("Assignees: zeus");
  });

  it("formats the wrapper block via formatIssueAttachment", () => {
    const issueAttachment: Attachment = {
      id: "att-issue",
      kind: "issue",
      ref: "#42",
      metadata: { label: "#42 Add dark mode", state: "open" },
      resolvedContent: buildIssueResolvedContent(makeIssueDetail()),
    };
    const block = buildAttachmentBlock([issueAttachment]);
    expect(block).not.toBeNull();
    expect(block).toContain("=== Attached context ===");
    expect(block).toContain("## Issue #42 Add dark mode [open]");
    expect(block).toContain("State: Open");
    expect(block).toContain("=== End context ===");
  });
});

describe("buildPrResolvedContent", () => {
  it("renders state, branches, author, diff stat, and body", () => {
    const out = buildPrResolvedContent(makePrDetail(), "");
    expect(out).toContain("State: OPEN");
    expect(out).toContain("URL: https://github.com/u/r/pull/42");
    expect(out).toContain("Author: alice");
    expect(out).toContain("Branches: main ← feat/dark");
    expect(out).toContain("Diff stat: +100 / -5");
    expect(out).toContain("Review: APPROVED");
    expect(out).toContain("Mergeable: MERGEABLE");
    expect(out).toContain("PR body here");
  });

  it("flags drafts in the state header", () => {
    const out = buildPrResolvedContent(
      makePrDetail({ is_draft: true }),
      "",
    );
    expect(out).toContain("State: OPEN (draft)");
  });

  it("renders name-only diff as a bulleted list with file count", () => {
    const diff = "src/foo.ts\nsrc/bar.ts\nREADME.md";
    const out = buildPrResolvedContent(makePrDetail(), diff);
    expect(out).toContain("### Files changed (3)");
    expect(out).toContain("- src/foo.ts");
    expect(out).toContain("- src/bar.ts");
    expect(out).toContain("- README.md");
    expect(out).toContain("Use `gh pr diff 42`");
  });

  it("renders a fenced unified diff when fullDiff option is set", () => {
    const diff = "diff --git a/x b/x\n@@ -1,1 +1,1 @@\n-old\n+new";
    const out = buildPrResolvedContent(makePrDetail(), diff, {
      fullDiff: true,
    });
    expect(out).toContain("### Diff (full)");
    expect(out).toContain("```diff");
    expect(out).toContain("diff --git a/x b/x");
    // Name-only files-changed list must NOT appear when fullDiff=true.
    expect(out).not.toContain("### Files changed");
  });

  it("falls back to '(no body)' when body is empty", () => {
    expect(
      buildPrResolvedContent(makePrDetail({ body: null }), ""),
    ).toContain("(no body)");
  });

  it("renders comments + truncation footer for long threads", () => {
    const comments = Array.from({ length: 20 }, (_, i) => ({
      author: `u${i}`,
      body: `c${i}`,
      createdAt: "2026-01-01T00:00:00Z",
    }));
    const out = buildPrResolvedContent(
      makePrDetail({ comments, totalComments: 51 }),
      "",
    );
    expect(out).toContain("(51 total, showing first 20)");
    expect(out).toContain(
      "31 more comments not shown. Use `gh pr view 42 --comments`",
    );
  });

  it("formats the wrapper block via formatPrAttachment", () => {
    const att: Attachment = {
      id: "att-pr",
      kind: "pr",
      ref: "!42",
      metadata: { label: "#42 Add dark mode", state: "open" },
      resolvedContent: buildPrResolvedContent(makePrDetail(), ""),
    };
    const block = buildAttachmentBlock([att]);
    expect(block).toContain("## Pull Request #42 Add dark mode [open]");
    expect(block).toContain("State: OPEN");
  });
});

describe("buildImageRefs (staged-image references)", () => {
  // The wire shape `agent_chat_send_turn` expects post-fix: each entry is
  // `{ path, media_type }` — a reference to the backend staging file the
  // bytes were written to at attach time. The raw `number[]` payload path
  // was deleted (it was the multi-minute first-send stall).
  function makeStagedImage(overrides: Partial<Attachment> = {}): Attachment {
    return {
      id: "img-1",
      kind: "image",
      ref: "image:img-1",
      metadata: { label: "screenshot.png" },
      resolvedImage: {
        mime: "image/png",
        bytes: new Uint8Array([1, 2, 3, 4]),
      },
      stagedImage: { path: "/staging/img-1.png", mediaType: "image/png" },
      ...overrides,
    };
  }

  it("returns empty array when no attachments are images", () => {
    const file: Attachment = {
      id: "f",
      kind: "file",
      ref: "x.ts",
      metadata: { label: "x.ts" },
      resolvedContent: "alpha",
    };
    expect(buildImageRefs([file])).toEqual([]);
  });

  it("maps a staged image to its { path, media_type } reference", () => {
    expect(buildImageRefs([makeStagedImage()])).toEqual([
      { path: "/staging/img-1.png", media_type: "image/png" },
    ]);
  });

  it("preserves order across multiple staged images", () => {
    const a = makeStagedImage({
      id: "a",
      stagedImage: { path: "/staging/a.png", mediaType: "image/png" },
    });
    const b = makeStagedImage({
      id: "b",
      resolvedImage: { mime: "image/jpeg", bytes: new Uint8Array([2]) },
      stagedImage: { path: "/staging/b.jpg", mediaType: "image/jpeg" },
    });
    expect(buildImageRefs([a, b])).toEqual([
      { path: "/staging/a.png", media_type: "image/png" },
      { path: "/staging/b.jpg", media_type: "image/jpeg" },
    ]);
  });

  it("skips a resolved image whose staging hasn't landed / failed", () => {
    // No `stagedImage` → excluded from the refs. Send-gating is the
    // caller's job (see unstagedImageAttachments) so a dropped image is
    // never silently sent.
    const unstaged = makeStagedImage({ id: "u", stagedImage: undefined });
    expect(buildImageRefs([unstaged])).toEqual([]);
  });

  it("skips images that are still loading (no resolvedImage)", () => {
    const loading: Attachment = {
      id: "loading",
      kind: "image",
      ref: "image:loading",
      metadata: { label: "pasting…", isLoading: true },
    };
    expect(buildImageRefs([loading])).toEqual([]);
  });

  it("ignores non-image attachments entirely", () => {
    const file: Attachment = {
      id: "f",
      kind: "file",
      ref: "x.ts",
      metadata: { label: "x.ts" },
      resolvedContent: "alpha",
    };
    const out = buildImageRefs([file, makeStagedImage()]);
    expect(out).toHaveLength(1);
    expect(out[0]?.media_type).toBe("image/png");
  });

  it("imageAttachmentIds returns ids of resolved image chips only", () => {
    const file: Attachment = {
      id: "f",
      kind: "file",
      ref: "x.ts",
      metadata: { label: "x.ts" },
    };
    const loading: Attachment = {
      id: "loading",
      kind: "image",
      ref: "image:loading",
      metadata: { label: "pasting…", isLoading: true },
    };
    expect(
      imageAttachmentIds([file, loading, makeStagedImage({ id: "img-1" })]),
    ).toEqual(["img-1"]);
  });

  it("unstagedImageAttachments flags resolved images missing a staged file", () => {
    const staged = makeStagedImage({ id: "ok" });
    const unstaged = makeStagedImage({ id: "bad", stagedImage: undefined });
    const out = unstagedImageAttachments([staged, unstaged]);
    expect(out.map((a) => a.id)).toEqual(["bad"]);
  });
});

describe("buildImageDisplaySources", () => {
  // The display shape the user-message bubble renders:
  // `{ src: data-URL, mediaType }`. Same resolved-image filter as
  // `buildImageRefs`, but base64-encodes into a self-contained
  // `data:` URL so the optimistic bubble shows thumbnails immediately.
  function makeImageAttachment(
    overrides: Partial<Attachment> = {},
  ): Attachment {
    return {
      id: "img-1",
      kind: "image",
      ref: "image:img-1",
      metadata: { label: "screenshot.png" },
      resolvedImage: {
        mime: "image/png",
        bytes: new Uint8Array([1, 2, 3, 4]),
      },
      ...overrides,
    };
  }

  it("builds a base64 data URL from the resolved bytes", () => {
    const out = buildImageDisplaySources([makeImageAttachment()]);
    // btoa of bytes [1,2,3,4] is "AQIDBA==".
    expect(out).toEqual([
      { src: "data:image/png;base64,AQIDBA==", mediaType: "image/png" },
    ]);
  });

  it("skips loading images and non-image attachments", () => {
    const loading: Attachment = {
      id: "loading",
      kind: "image",
      ref: "image:loading",
      metadata: { label: "pasting…", isLoading: true },
    };
    const file: Attachment = {
      id: "f",
      kind: "file",
      ref: "x.ts",
      metadata: { label: "x.ts" },
      resolvedContent: "alpha",
    };
    expect(buildImageDisplaySources([loading, file])).toEqual([]);
  });

  it("preserves order and per-image mime across multiple images", () => {
    const a = makeImageAttachment({
      id: "a",
      resolvedImage: { mime: "image/png", bytes: new Uint8Array([0]) },
    });
    const b = makeImageAttachment({
      id: "b",
      resolvedImage: { mime: "image/jpeg", bytes: new Uint8Array([255]) },
    });
    const out = buildImageDisplaySources([a, b]);
    expect(out.map((s) => s.mediaType)).toEqual(["image/png", "image/jpeg"]);
    expect(out[0]?.src.startsWith("data:image/png;base64,")).toBe(true);
    expect(out[1]?.src.startsWith("data:image/jpeg;base64,")).toBe(true);
  });

  it("encodes a large image without a call-stack overflow", () => {
    // 256 KB — well past the ~64K single-call argument ceiling that a
    // naive `String.fromCharCode(...bytes)` spread would blow. The
    // chunked encoder must handle it.
    const big = new Uint8Array(256 * 1024);
    for (let i = 0; i < big.length; i++) big[i] = i % 256;
    const out = buildImageDisplaySources([
      makeImageAttachment({ resolvedImage: { mime: "image/png", bytes: big } }),
    ]);
    expect(out).toHaveLength(1);
    // Round-trip a prefix to confirm the base64 is valid, not garbage.
    const b64 = out[0]!.src.split(",")[1]!;
    const decoded = atob(b64);
    expect(decoded.charCodeAt(0)).toBe(0);
    expect(decoded.charCodeAt(1)).toBe(1);
    expect(decoded.length).toBe(big.length);
  });
});
