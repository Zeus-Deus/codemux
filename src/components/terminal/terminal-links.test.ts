import { describe, expect, it } from "vitest";
import type { Terminal } from "@xterm/xterm";

import {
  findTerminalLinks,
  isLoopbackUrl,
  rangeForMatch,
  readLogicalLine,
  resolveTerminalPath,
} from "./terminal-links";

describe("findTerminalLinks", () => {
  it("finds URLs and drops sentence punctuation after them", () => {
    const links = findTerminalLinks("Local: http://localhost:5173/, see https://github.com/o/r/pull/12.");
    expect(links.map((l) => l.kind === "url" && l.url)).toEqual([
      "http://localhost:5173/",
      "https://github.com/o/r/pull/12",
    ]);
  });

  it("keeps a closing paren that belongs to the URL", () => {
    const [link] = findTerminalLinks("(see https://en.wikipedia.org/wiki/Foo_(bar))");
    expect(link).toMatchObject({ kind: "url", url: "https://en.wikipedia.org/wiki/Foo_(bar)" });
  });

  it("finds file references with line and column", () => {
    const text = "error at src/foo.ts:42:7 and ./lib/a.rs:3, also /abs/x.py";
    const files = findTerminalLinks(text).filter((l) => l.kind === "file");
    expect(files).toEqual([
      { kind: "file", start: 9, end: 24, path: "src/foo.ts", line: 42, column: 7 },
      { kind: "file", start: 29, end: 41, path: "./lib/a.rs", line: 3, column: undefined },
      { kind: "file", start: 48, end: 57, path: "/abs/x.py", line: undefined, column: undefined },
    ]);
  });

  it("reads tsc's parenthesised positions", () => {
    const [link] = findTerminalLinks("src/app.tsx(12,5): error TS2322");
    expect(link).toMatchObject({ kind: "file", path: "src/app.tsx", line: 12, column: 5 });
  });

  it("ignores prose that only looks like a filename", () => {
    expect(findTerminalLinks("Built with Node.js and e.g. v1.2.3 in 12:30")).toEqual([]);
  });

  it("accepts a bare filename when it carries a line number", () => {
    expect(findTerminalLinks("main.go:10")).toMatchObject([
      { kind: "file", path: "main.go", line: 10 },
    ]);
  });

  it("does not report a path inside a URL as a file", () => {
    expect(findTerminalLinks("https://example.com/docs/a.html:3").map((l) => l.kind)).toEqual([
      "url",
    ]);
  });
});

describe("resolveTerminalPath", () => {
  it("resolves relative paths against the cwd", () => {
    expect(resolveTerminalPath("./src/../lib/a.ts", "/repo/app")).toBe("/repo/app/lib/a.ts");
  });

  it("keeps absolute paths and refuses relative ones without a cwd", () => {
    expect(resolveTerminalPath("/etc/hosts", null)).toBe("/etc/hosts");
    expect(resolveTerminalPath("src/a.ts", null)).toBeNull();
  });
});

describe("isLoopbackUrl", () => {
  it("recognises dev-server hosts", () => {
    expect(isLoopbackUrl("http://localhost:3000")).toBe(true);
    expect(isLoopbackUrl("http://127.0.0.1:8080/x")).toBe(true);
    expect(isLoopbackUrl("http://[::1]:4000")).toBe(true);
    expect(isLoopbackUrl("http://app.localhost")).toBe(true);
    expect(isLoopbackUrl("https://github.com")).toBe(false);
  });
});

/** A buffer of fixed-width rows; `wide` marks two-cell glyphs. */
function fakeTerminal(rows: { cells: string[]; wrapped?: boolean }[]) {
  const lines = rows.map((row) => ({
    isWrapped: !!row.wrapped,
    length: row.cells.length,
    getCell: (x: number) => {
      const chars = row.cells[x];
      return {
        getChars: () => (chars === "\u0000" ? "" : chars),
        getWidth: () => (chars === "\u0000" ? 0 : /\p{Extended_Pictographic}/u.test(chars) ? 2 : 1),
      };
    },
  }));
  return {
    buffer: { active: { getLine: (y: number) => lines[y] } },
  } as unknown as Pick<Terminal, "buffer">;
}

describe("readLogicalLine", () => {
  it("joins wrapped rows and maps wide glyphs to their cells", () => {
    // "✅ a.ts:1" where ✅ takes two cells (the second is a width-0 spacer),
    // wrapped across two rows of five cells.
    const term = fakeTerminal([
      { cells: ["✅", "\u0000", " ", "a", "."] },
      { cells: ["t", "s", ":", "1", " "], wrapped: true },
    ]);
    const { text, cells } = readLogicalLine(term, 1);
    expect(text).toBe("✅ a.ts:1 ");
    const [link] = findTerminalLinks(text);
    expect(link).toMatchObject({ kind: "file", path: "a.ts", line: 1 });
    expect(rangeForMatch(cells, link.start, link.end)).toEqual({
      start: { x: 4, y: 1 },
      end: { x: 4, y: 2 },
    });
  });
});
