import { describe, expect, it } from "vitest";

import { resolveChatFileLink } from "./file-links";

describe("resolveChatFileLink", () => {
  const cwd = "/work/codemux";

  it.each([
    ["src/components/Chat.tsx", "/work/codemux/src/components/Chat.tsx", undefined, undefined],
    ["src/components/Chat.tsx:42", "/work/codemux/src/components/Chat.tsx", 42, undefined],
    ["src/components/Chat.tsx:42:7", "/work/codemux/src/components/Chat.tsx", 42, 7],
    ["src/components/Chat.tsx#L9C3", "/work/codemux/src/components/Chat.tsx", 9, 3],
    ["src/My%20File.ts:11", "/work/codemux/src/My File.ts", 11, undefined],
    ["/work/codemux/src/lib.ts:5", "/work/codemux/src/lib.ts", 5, undefined],
    ["/work/another-project/AGENTS.md", "/work/another-project/AGENTS.md", undefined, undefined],
    ["WORKFLOW.md", "/work/codemux/WORKFLOW.md", undefined, undefined],
  ])("resolves source reference %s", (input, filePath, line, column) => {
    const resolved = resolveChatFileLink(input as string, cwd);
    expect(resolved?.filePath).toBe(filePath);
    expect(resolved?.line).toBe(line);
    expect(resolved?.column).toBe(column);
  });

  it("supports Windows worktree references without treating the drive as a URL scheme", () => {
    expect(resolveChatFileLink("C:\\repo\\src\\main.rs:18", "C:\\repo")).toMatchObject({
      filePath: "C:/repo/src/main.rs",
      line: 18,
    });
  });

  it.each([
    "https://example.com/file.ts",
    "mailto:dev@example.com",
    "../outside.ts",
    "package@1.2.3",
    "v1.2.3",
    "not a source reference",
  ])("rejects non-worktree target %s", (input) => {
    expect(resolveChatFileLink(input, cwd)).toBeNull();
  });

  describe("allowSpaces: false (inline-code sources)", () => {
    it.each([
      "cargo check --manifest-path src-tauri/Cargo.toml",
      "npx vitest run src/components/chat/ChatMarkdown.links.test.tsx",
      "cat src/foo.ts",
      "src/My File.ts",
      "src/My%20File.ts:11",
    ])("rejects the command-shaped span %s", (input) => {
      expect(resolveChatFileLink(input, cwd, { allowSpaces: false })).toBeNull();
    });

    it("still resolves a bare whitespace-free reference", () => {
      expect(
        resolveChatFileLink("src/components/Chat.tsx:42", cwd, {
          allowSpaces: false,
        }),
      ).toMatchObject({ filePath: "/work/codemux/src/components/Chat.tsx", line: 42 });
    });

    it("keeps percent-encoded spaces resolvable for href sources", () => {
      expect(resolveChatFileLink("src/My%20File.ts:11", cwd)).toMatchObject({
        filePath: "/work/codemux/src/My File.ts",
        line: 11,
      });
    });
  });
});
