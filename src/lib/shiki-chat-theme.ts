import type { ThemeRegistrationAny } from "shiki";
import type { ThemeColors } from "@/tauri/types";

/**
 * Builds a Shiki theme from the active terminal ANSI palette so fenced
 * code blocks in chat are colored by the same source as the file editor
 * (`src/lib/codemirror-theme.ts`) and the terminal panes.
 *
 * This is the documented ANSI-palette exception to the no-hardcoded-colors
 * rule (see `docs/reference/DESIGN-SYSTEM.md`): Shiki emits concrete colors
 * into inline `style` attributes, so it needs real values rather than CSS
 * variables. Structural chrome (container, header, borders) stays on design
 * tokens — only the token colors come from `ThemeColors`.
 *
 * The scope map below mirrors the Lezer tag map in `codemirror-theme.ts`
 * so a given construct is the same color in chat and in the editor. Keep
 * the two in sync when either changes.
 */

/** FNV-1a (32-bit), matching the hashing style used elsewhere for cache keys. */
function fnv1a32(input: string): number {
  let hash = 0x811c9dc5;
  for (let i = 0; i < input.length; i += 1) {
    hash ^= input.charCodeAt(i);
    // 32-bit FNV prime multiply via shifts, kept in uint32 range.
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash >>> 0;
}

/**
 * Shiki's highlighter cache inside `@streamdown/code` is keyed by theme
 * *name*, not by theme content. A fixed name would serve stale colors after
 * the user switches terminal theme, so the palette is hashed into the name:
 * a new palette is a new theme identity and misses the cache correctly.
 */
function paletteId(theme: ThemeColors): string {
  const palette = [
    theme.foreground,
    theme.background,
    theme.color1,
    theme.color2,
    theme.color3,
    theme.color4,
    theme.color5,
    theme.color6,
    theme.color8,
    theme.color11,
  ].join("|");
  return fnv1a32(palette).toString(36);
}

function buildTokenColors(theme: ThemeColors) {
  return [
    {
      scope: ["keyword", "storage", "storage.type", "storage.modifier", "keyword.control"],
      settings: { foreground: theme.color5 },
    },
    {
      scope: ["comment", "punctuation.definition.comment", "string.comment"],
      settings: { foreground: theme.color8, fontStyle: "italic" },
    },
    {
      scope: ["string", "string.quoted", "string.template", "constant.other.symbol"],
      settings: { foreground: theme.color2 },
    },
    {
      scope: ["constant.numeric", "constant.numeric.integer", "constant.numeric.float"],
      settings: { foreground: theme.color3 },
    },
    {
      scope: [
        "entity.name.function",
        "support.function",
        "meta.function-call",
        "variable.function",
      ],
      settings: { foreground: theme.color4 },
    },
    {
      scope: [
        "entity.name.type",
        "entity.name.class",
        "entity.name.namespace",
        "support.type",
        "support.class",
      ],
      settings: { foreground: theme.color6 },
    },
    {
      scope: ["keyword.operator", "punctuation", "meta.brace"],
      settings: { foreground: theme.color1 },
    },
    {
      scope: ["constant.language", "constant.language.boolean", "constant.language.null"],
      settings: { foreground: theme.color3 },
    },
    {
      scope: [
        "variable.other.property",
        "support.variable.property",
        "meta.object-literal.key",
      ],
      settings: { foreground: theme.color4 },
    },
    {
      scope: ["variable", "variable.other", "meta.definition.variable.name"],
      settings: { foreground: theme.foreground },
    },
    {
      scope: ["meta.annotation", "entity.name.decorator", "meta.decorator"],
      settings: { foreground: theme.color11 },
    },
    {
      scope: ["entity.name.tag", "punctuation.definition.tag"],
      settings: { foreground: theme.color1 },
    },
    {
      scope: ["entity.other.attribute-name"],
      settings: { foreground: theme.color3 },
    },
    {
      scope: ["markup.heading", "entity.name.section"],
      settings: { foreground: theme.color4, fontStyle: "bold" },
    },
    {
      scope: ["markup.underline.link", "markup.link", "string.other.link"],
      settings: { foreground: theme.color6, fontStyle: "underline" },
    },
    {
      scope: ["markup.inserted", "meta.diff.header.to-file"],
      settings: { foreground: theme.color2 },
    },
    {
      scope: ["markup.deleted", "meta.diff.header.from-file"],
      settings: { foreground: theme.color1 },
    },
    {
      scope: ["markup.changed", "meta.diff.range"],
      settings: { foreground: theme.color3 },
    },
    {
      scope: ["invalid", "invalid.illegal"],
      settings: { foreground: theme.color9 },
    },
  ];
}

/**
 * Streamdown always renders a light/dark theme pair, and Shiki emits both a
 * `color` and a `--shiki-dark` value per token. The app shell is dark-only
 * (`main.tsx` pins `.dark` on the root) and the terminal ANSI palette is a
 * single palette — the CodeMirror editor theme likewise hardcodes
 * `{ dark: true }` — so both slots are filled with the same colors under
 * distinct names. Distinct names matter because Shiki registers themes by
 * name and the pair must not collide.
 *
 * The background is transparent so the code-block container's own token-based
 * surface shows through.
 */
export function buildChatCodeThemes(
  theme: ThemeColors,
): [ThemeRegistrationAny, ThemeRegistrationAny] {
  const id = paletteId(theme);
  const tokenColors = buildTokenColors(theme);

  const base = {
    bg: "transparent",
    fg: theme.foreground,
    tokenColors,
  } as const;

  return [
    { ...base, name: `codemux-ansi-${id}-light`, type: "light" },
    { ...base, name: `codemux-ansi-${id}-dark`, type: "dark" },
  ];
}
