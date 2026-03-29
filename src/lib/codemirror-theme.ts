import { EditorView } from "@codemirror/view";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags } from "@lezer/highlight";
import type { Extension } from "@codemirror/state";
import type { ThemeColors } from "@/tauri/types";

/**
 * Structural/chrome colors use CSS variables (same source as terminal panes)
 * so the editor background always matches the rest of the app.
 * Syntax highlighting uses ThemeColors ANSI palette for token colors.
 */
export function buildEditorTheme(theme: ThemeColors): Extension[] {
  const editorTheme = EditorView.theme(
    {
      "&": {
        backgroundColor: "var(--background)",
        color: "var(--foreground)",
      },
      ".cm-content": {
        caretColor: "var(--sidebar-primary)",
        fontFamily: "'JetBrains Mono Variable', monospace",
        fontSize: "13px",
        lineHeight: "1.6",
      },
      ".cm-cursor, .cm-dropCursor": {
        borderLeftColor: "var(--sidebar-primary)",
      },
      "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": {
        backgroundColor: "var(--accent)",
      },
      ".cm-activeLine": {
        backgroundColor: "color-mix(in srgb, var(--accent) 15%, transparent)",
      },
      ".cm-gutters": {
        backgroundColor: "var(--background)",
        color: "var(--muted-foreground)",
        borderRight: "none",
      },
      ".cm-activeLineGutter": {
        backgroundColor: "color-mix(in srgb, var(--accent) 15%, transparent)",
        color: "var(--foreground)",
      },
      ".cm-lineNumbers .cm-gutterElement": {
        padding: "0 8px 0 16px",
        minWidth: "3em",
      },
      ".cm-foldPlaceholder": {
        backgroundColor: "transparent",
        border: "none",
        color: "var(--muted-foreground)",
      },
      ".cm-tooltip": {
        backgroundColor: "var(--card)",
        color: "var(--foreground)",
        border: "1px solid var(--border)",
      },
      ".cm-panels": {
        backgroundColor: "var(--card)",
        color: "var(--foreground)",
      },
      ".cm-panels.cm-panels-top": {
        borderBottom: "1px solid var(--border)",
      },
      ".cm-searchMatch": {
        backgroundColor: `${theme.color3}30`,
      },
      ".cm-searchMatch.cm-searchMatch-selected": {
        backgroundColor: `${theme.color3}60`,
      },
    },
    { dark: true },
  );

  const highlighting = syntaxHighlighting(
    HighlightStyle.define([
      { tag: [tags.keyword, tags.operatorKeyword, tags.modifier], color: theme.color5 },
      { tag: [tags.comment, tags.lineComment, tags.blockComment], color: theme.color8, fontStyle: "italic" },
      { tag: [tags.string, tags.special(tags.string)], color: theme.color2 },
      { tag: [tags.number, tags.integer, tags.float], color: theme.color3 },
      { tag: [tags.function(tags.variableName), tags.function(tags.propertyName)], color: theme.color4 },
      { tag: [tags.typeName, tags.className, tags.namespace], color: theme.color6 },
      { tag: [tags.operator, tags.punctuation], color: theme.color1 },
      { tag: [tags.bool, tags.null, tags.atom], color: theme.color3 },
      { tag: [tags.propertyName], color: theme.color4 },
      { tag: [tags.variableName], color: theme.foreground },
      { tag: [tags.meta, tags.annotation], color: theme.color11 },
      { tag: [tags.tagName], color: theme.color1 },
      { tag: [tags.attributeName], color: theme.color3 },
      { tag: [tags.heading], color: theme.color4, fontWeight: "bold" },
      { tag: [tags.link, tags.url], color: theme.color6, textDecoration: "underline" },
    ]),
  );

  return [editorTheme, highlighting];
}
