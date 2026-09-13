import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { describe, expect, it } from "vitest";

import { buildEditorTheme, buildEditorThemeSpec } from "./codemirror-theme";
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

/** Graphite Light's palette: the same ANSI shape over a white canvas. */
const lightPalette: ThemeColors = {
  ...palette,
  accent: "#c2410c",
  cursor: "#c2410c",
  foreground: "#1c1917",
  background: "#ffffff",
  selection_foreground: "#1c1917",
  selection_background: "#d6d3d1",
  color0: "#1c1917",
  color7: "#d6d3d1",
  color8: "#78716c",
  color15: "#ffffff",
};

const globalsCss = readFileSync(resolve(process.cwd(), "src/globals.css"), "utf8");

describe("editor selection contract", () => {
  it("uses light editor defaults for a light desktop palette", () => {
    const state = EditorState.create({ extensions: buildEditorTheme({ ...palette, background: "#eff1f5" }) });
    expect(state.facet(EditorView.darkTheme)).toBe(false);
  });
  it("keeps syntax colors on selected code instead of the app selection ink", () => {
    const line = buildEditorThemeSpec(palette)[".cm-line"] as Record<string, Record<string, string>>;

    // `drawSelection()` paints its own selection layer and only resets the
    // native highlight's background, so the editor has to opt its foreground
    // out of the document-level `:root::selection` pair explicitly.
    expect(line["&::selection, & ::selection"]).toEqual({ color: "currentColor" });
  });

  it("draws its own selection layer from the accent token", () => {
    const spec = buildEditorThemeSpec(palette);
    expect(spec["&.cm-editor .cm-selectionBackground"]).toEqual({
      backgroundColor: "var(--accent)",
    });
  });

  // CodeMirror's base theme paints the *focused* layer through a five-class
  // selector. Without an equally qualified rule the editor keeps the stock
  // lilac/slate selection and the palette never reaches the one state the
  // user is actually in while selecting.
  it("out-qualifies CodeMirror's own focused-selection default", () => {
    const focused =
      "&.cm-editor.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground";
    for (const theme of [palette, lightPalette]) {
      const spec = buildEditorThemeSpec(theme);
      expect(spec[focused]).toEqual(spec["&.cm-editor .cm-selectionBackground"]);
    }
  });
});

/** Reads the `dark` flag back off the built extension the way CodeMirror does. */
function isDark(theme: ThemeColors): boolean {
  return EditorState.create({ extensions: buildEditorTheme(theme) }).facet(
    EditorView.darkTheme,
  );
}

const selectionOf = (theme: ThemeColors) =>
  (
    buildEditorThemeSpec(theme)["&.cm-editor .cm-selectionBackground"] as Record<
      string,
      string
    >
  ).backgroundColor;

describe("scheme", () => {
  it("follows the palette's canvas rather than a hardcoded dark", () => {
    expect(isDark(palette)).toBe(true);
    expect(isDark(lightPalette)).toBe(false);
  });

  it("keeps the selection off the canvas on a light palette", () => {
    // `--accent` on a light palette is stone-50 over white — a selection
    // painted with it is invisible, so the light branch mixes ink into the
    // canvas instead of reaching for the surface token.
    expect(selectionOf(palette)).toBe("var(--accent)");
    const light = selectionOf(lightPalette);
    expect(light).not.toBe("var(--accent)");
    expect(light).toContain("var(--foreground)");
    expect(light).toContain("var(--background)");
  });

  it("washes the active line away from the canvas in both schemes", () => {
    const activeLine = (theme: ThemeColors) =>
      (buildEditorThemeSpec(theme)[".cm-activeLine"] as Record<string, string>)
        .backgroundColor;
    expect(activeLine(palette)).toBe(
      "color-mix(in srgb, var(--accent) 15%, transparent)",
    );
    expect(activeLine(lightPalette)).toBe(
      "color-mix(in srgb, var(--foreground) 4%, transparent)",
    );
  });
});

describe("document selection contract", () => {
  it("replaces WebKit transcript box paint only when the text-range highlight is active", () => {
    const nativeRule = globalsCss.match(
      /\.transcript-selection-highlight \[data-slot="transcript-list"\] \*::selection\s*\{([^}]*)\}/,
    )?.[1] ?? "";
    const customRule = globalsCss.match(
      /::highlight\(codemux-transcript-selection\)\s*\{([^}]*)\}/,
    )?.[1] ?? "";

    expect(nativeRule).toContain("background-color: transparent");
    expect(nativeRule).toContain("color: currentColor");
    expect(customRule).toContain(
      "background-color: var(--selection-background)",
    );
    expect(customRule).toContain("color: var(--selection-foreground)");
  });

  it("leaves text controls in the transcript with a visible native selection", () => {
    // A textarea's editable text has no light-DOM text nodes, so the custom
    // highlight can never paint it. Without this rule the suppression above
    // inherits in and selecting a typed deny reason shows nothing at all.
    const textareaRule = globalsCss.match(
      /\.transcript-selection-highlight\s+\[data-slot="transcript-list"\]\s+textarea::selection[^{]*\{([^}]*)\}/,
    )?.[1] ?? "";

    expect(textareaRule).toContain("background-color: var(--selection-background)");
    expect(textareaRule).toContain("color: var(--selection-foreground)");
  });

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
