import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

// jsdom cannot exercise WebKit's deferred layout. Keep the CSS measurement
// contract covered here; the native scroll fixture covers its frame cost.
describe("virtualized chat code-card sizing", () => {
  it("lets LegendList measure real code-card heights without deferred placeholders", () => {
    const css = readFileSync(resolve(process.cwd(), "src/globals.css"), "utf8");
    const card = css.match(/\.chat-markdown \[data-chat-code-block\]\s*\{([^}]+)\}/)?.[1];
    expect(card).toBeDefined();
    expect(card).not.toMatch(/content-visibility\s*:\s*auto/);
    expect(card).not.toMatch(/contain-intrinsic-size\s*:/);
  });
});
