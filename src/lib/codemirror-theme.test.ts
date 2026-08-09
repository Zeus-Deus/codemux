import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import { buildEditorThemeSpec } from "./codemirror-theme";
import type { ThemeColors } from "@/tauri/types";

const palette: ThemeColors = {
  accent: "#7aa2f7",
  cursor: "#c0caf5",
  foreground: "#c0caf5",
  background: "#1a1b26",
  selection_foreground: "#c0caf5",
  selection_background: "#283457",
  color0: "#15161e",
  color1: "#f7768e",
  color2: "#9ece6a",
  color3: "#e0af68",
  color4: "#7aa2f7",
  color5: "#bb9af7",
  color6: "#7dcfff",
  color7: "#a9b1d6",
  color8: "#414868",
  color9: "#ff0000",
  color10: "#9ece6a",
  color11: "#e0af68",
  color12: "#7aa2f7",
  color13: "#bb9af7",
  color14: "#7dcfff",
  color15: "#c0caf5",
};

const globalsCss = readFileSync(resolve(process.cwd(), "src/globals.css"), "utf8");

describe("editor selection contract", () => {
  it("keeps syntax colors on selected code instead of the app selection ink", () => {
    const line = buildEditorThemeSpec(palette)[".cm-line"] as Record<string, Record<string, string>>;

    // `drawSelection()` paints its own selection layer and only resets the
    // native highlight's background, so the editor has to opt its foreground
    // out of the document-level `:root::selection` pair explicitly.
    expect(line["&::selection, & ::selection"]).toEqual({ color: "currentColor" });
  });

  it("draws its own selection layer from the accent token", () => {
    const spec = buildEditorThemeSpec(palette);
    expect(spec["&.cm-focused .cm-selectionBackground, .cm-selectionBackground"]).toEqual({
      backgroundColor: "var(--accent)",
    });
  });
});

describe("document selection contract", () => {
  it("defines both selection channels at the root so no native fallback leaks in", () => {
    const rule = globalsCss.match(/:root::selection\s*\{([^}]*)\}/)?.[1] ?? "";
    expect(rule).toContain("background-color: var(--selection-background)");
    expect(rule).toContain("color: var(--selection-foreground)");
  });

  it("keeps the selection pair as dedicated tokens rather than palette foreground", () => {
    expect(globalsCss).toMatch(/--selection-background:\s*oklch\(/);
    expect(globalsCss).toMatch(/--selection-foreground:\s*oklch\(/);
  });
});
