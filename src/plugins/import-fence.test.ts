/**
 * The bundled-plugin fence (Hermes enforces the same rule with ESLint):
 * code under src/plugins/ may import only @codemux/sdk, react*, and files in
 * its own folder (`./x`). If a plugin needs something else, add it to the
 * SDK instead.
 */
import { describe, expect, it } from "vitest";
import { unsupportedImports } from "@/contrib/import-lexer";

const sources = import.meta.glob<string>("./**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
});

const PACKAGES = /^(@codemux\/sdk|react|react\/jsx-runtime|react\/jsx-dev-runtime|react-dom)$/;
/** A sibling module in the plugin's own folder: `./name`, no `..`, no subfolder
 *  escape and no asset suffix, so a plugin can split its code into files. */
const OWN_FILE = /^\.\/[\w.-]+$/;
const ALLOWED = { test: (spec: string) => PACKAGES.test(spec) || (OWN_FILE.test(spec) && !spec.includes("..") && !/\.(webp|png|jpe?g|svg|gif|css)$/.test(spec)) };

describe("bundled plugin import fence", () => {
  const files = Object.entries(sources).filter(([path]) => !path.endsWith(".test.ts") && !path.endsWith(".test.tsx"));

  it("finds the bundled plugins", () => {
    expect(files.length).toBeGreaterThan(0);
  });

  it.each(files)("%s imports only @codemux/sdk, react and its own files", (_path, source) => {
    // `import type` is erased, but keep the rule simple: no @/ or ../ at all.
    expect(unsupportedImports(source, (spec) => ALLOWED.test(spec))).toEqual([]);
  });

  it("accepts sibling files and rejects everything that leaves the plugin", () => {
    const ok = (spec: string) => ALLOWED.test(spec);
    expect(["@codemux/sdk", "react", "./field", "./Backdrop"].every(ok)).toBe(true);
    expect(["../example/plugin", "./../x", "@/lib/plates", "./art.webp", "lodash", "./a/b"].some(ok)).toBe(false);
  });
});
