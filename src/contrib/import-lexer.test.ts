import { describe, expect, it } from "vitest";
import { rewriteSpecifiers, unsupportedImports } from "./import-lexer";

const allowed = (s: string) => s === "@codemux/sdk" || s === "react" || s === "react/jsx-runtime";

describe("import lexer", () => {
  it("finds static, side-effect and dynamic imports in code", () => {
    const src = `import a from "x"; import "y"; const m = import('z'); import { b } from "@codemux/sdk";`;
    expect(unsupportedImports(src, allowed).sort()).toEqual(["x", "y", "z"]);
  });

  it("ignores import-looking text in strings, templates, comments and regexes", () => {
    const src = [
      `const label = 'Copy keys from "x"';`,
      "const t = `import 'y'`;",
      `// import "z"`,
      `/* from "w" */`,
      `const re = /from "q"/;`,
      `import { host } from "@codemux/sdk";`,
    ].join("\n");
    expect(unsupportedImports(src, allowed)).toEqual([]);
  });

  it("treats template interpolations as code", () => {
    const src = "const t = `${await import('evil')}`;";
    expect(unsupportedImports(src, allowed)).toEqual(["evil"]);
  });

  it("refuses relative paths and URLs", () => {
    expect(unsupportedImports(`import x from "./local.js"; import y from "https://e.com/a.js";`, allowed)).toEqual([
      "./local.js",
      "https://e.com/a.js",
    ]);
  });

  it("rewrites only in-code mapped specifiers", () => {
    const src = `import { host } from "@codemux/sdk";\nconst s = "from '@codemux/sdk'";`;
    const out = rewriteSpecifiers(src, { "@codemux/sdk": "blob:sdk" });
    expect(out).toContain(`from "blob:sdk"`);
    expect(out).toContain(`"from '@codemux/sdk'"`);
  });

  it("keeps division apart from regex literals", () => {
    const src = `const a = b / 2; const c = d / e; import x from "bad";`;
    expect(unsupportedImports(src, allowed)).toEqual(["bad"]);
  });
});
