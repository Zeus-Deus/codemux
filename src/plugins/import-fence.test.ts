/**
 * The bundled-plugin fence (Hermes enforces the same rule with ESLint):
 * code under src/plugins/ may import only @codemux/sdk and react*. If a
 * plugin needs something else, add it to the SDK instead.
 */
import { describe, expect, it } from "vitest";
import { unsupportedImports } from "@/contrib/import-lexer";

const sources = import.meta.glob<string>("./**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
});

const ALLOWED = /^(@codemux\/sdk|react|react\/jsx-runtime|react\/jsx-dev-runtime|react-dom)$/;

describe("bundled plugin import fence", () => {
  const files = Object.entries(sources).filter(([path]) => !path.endsWith(".test.ts") && !path.endsWith(".test.tsx"));

  it("finds the bundled plugins", () => {
    expect(files.length).toBeGreaterThan(0);
  });

  it.each(files)("%s imports only @codemux/sdk and react", (_path, source) => {
    // `import type` is erased, but keep the rule simple: no @/ or ../ at all.
    expect(unsupportedImports(source, (spec) => ALLOWED.test(spec))).toEqual([]);
  });
});
