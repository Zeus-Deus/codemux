import { describe, expect, it } from "vitest";

import { buildChatCodeThemes } from "./shiki-chat-theme";
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

/** Flattens the token map to `scope -> foreground` for easy assertions. */
function foregroundFor(theme: ReturnType<typeof buildChatCodeThemes>[number], scope: string) {
  const entry = (
    theme as unknown as {
      tokenColors: { scope: string[]; settings: { foreground?: string } }[];
    }
  ).tokenColors.find((t) => t.scope.includes(scope));
  return entry?.settings.foreground;
}

describe("buildChatCodeThemes", () => {
  it("returns a light/dark pair with distinct names", () => {
    const [light, dark] = buildChatCodeThemes(palette);
    expect(light.type).toBe("light");
    expect(dark.type).toBe("dark");
    expect(light.name).not.toBe(dark.name);
  });

  it("maps token scopes to the ANSI palette, matching the editor theme", () => {
    const [theme] = buildChatCodeThemes(palette);
    // Mirrors src/lib/codemirror-theme.ts so chat and the editor agree.
    expect(foregroundFor(theme, "keyword")).toBe(palette.color5);
    expect(foregroundFor(theme, "comment")).toBe(palette.color8);
    expect(foregroundFor(theme, "string")).toBe(palette.color2);
    expect(foregroundFor(theme, "constant.numeric")).toBe(palette.color3);
    expect(foregroundFor(theme, "entity.name.function")).toBe(palette.color4);
    expect(foregroundFor(theme, "entity.name.type")).toBe(palette.color6);
  });

  it("keeps the background transparent so the card surface shows through", () => {
    for (const theme of buildChatCodeThemes(palette)) {
      expect(theme.bg).toBe("transparent");
      expect(theme.fg).toBe(palette.foreground);
    }
  });

  it("italicizes comments", () => {
    const [theme] = buildChatCodeThemes(palette);
    const comment = (
      theme as unknown as {
        tokenColors: { scope: string[]; settings: { fontStyle?: string } }[];
      }
    ).tokenColors.find((t) => t.scope.includes("comment"));
    expect(comment?.settings.fontStyle).toBe("italic");
  });

  // The highlighter cache in `@streamdown/code` is keyed by theme *name*, so a
  // palette change must produce a new name or the old colors are served back.
  it("changes the theme name when the palette changes", () => {
    const [a] = buildChatCodeThemes(palette);
    const [b] = buildChatCodeThemes({ ...palette, color5: "#123456" });
    expect(a.name).not.toBe(b.name);
  });

  it("is stable for an unchanged palette", () => {
    const [a] = buildChatCodeThemes(palette);
    const [b] = buildChatCodeThemes({ ...palette });
    expect(a.name).toBe(b.name);
  });
});
