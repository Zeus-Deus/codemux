import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

import {
  ANSI_SLOTS,
  BUILT_IN_THEMES,
  THEME_ROLES,
  THEME_ROLE_VARIABLES,
  contrastRatio,
  createGeneratedTheme,
  importThemeDetailed,
  importThemeText,
  normalizeColor,
  parseCustomTheme,
  serializeTheme,
} from "./themes";

describe("theme registry", () => {
  it("ships complete, unique, dark built-ins", () => {
    expect(new Set(BUILT_IN_THEMES.map((theme) => theme.id)).size).toBe(BUILT_IN_THEMES.length);
    for (const theme of BUILT_IN_THEMES) {
      expect(theme.scheme).toBe("dark");
      expect(Object.keys(theme.roles).sort()).toEqual([...THEME_ROLES].sort());
      expect(Object.keys(theme.ansi).sort()).toEqual([...ANSI_SLOTS].sort());
    }
  });

  it("generates a complete contrast-solved palette from two colors", () => {
    const theme = createGeneratedTheme("Signal", "#10171b", "#e07850");
    expect(theme.managed).toBe(true);
    expect(theme.roles.background).toBe("#10171b");
    expect(contrastRatio(theme.roles.foreground, theme.roles.background)).toBeGreaterThanOrEqual(7);
    expect(contrastRatio(theme.roles.mutedForeground, theme.roles.background)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(theme.roles.primaryForeground, theme.roles.primary)).toBeGreaterThanOrEqual(4.5);
    expect(Object.keys(theme.ansi)).toHaveLength(16);
  });

  it("rejects a light canvas until the app shell supports light mode", () => {
    expect(() => createGeneratedTheme("Day", "#ffffff", "#0066cc")).toThrow(/Light themes/);
  });

  it("flattens hex and OKLCH alpha over the resolved background", () => {
    expect(normalizeColor("#ffffff80", "#000000")).toBe("#808080");
    expect(normalizeColor("oklch(1 0 0 / 50%)", "#000000")).toBe("#808080");
  });

  it("round-trips versioned JSON and completes partial files", () => {
    const generated = createGeneratedTheme("Round trip", "#11161a", "#72c7cf");
    expect(parseCustomTheme(JSON.parse(serializeTheme(generated)))?.ansi).toEqual(generated.ansi);

    const partial = parseCustomTheme({
      version: 1,
      id: "custom-partial",
      label: "Partial",
      scheme: "dark",
      roles: { background: "#101010", primary: "#ff9955" },
    });
    expect(partial?.roles.background).toBe("#101010");
    expect(partial?.roles.card).toMatch(/^#/);
    expect(Object.keys(partial?.ansi ?? {})).toHaveLength(16);
  });
});

describe("theme import pipeline", () => {
  it("maps a shadcn variable block and accepts its radius", () => {
    const theme = importThemeText(`
      .dark {
        --background: #10151a;
        --foreground: #f1f5f7;
        --primary: #73c7d3;
        --primary-foreground: #081012;
        --card: #182128;
        --sidebar: #0b1115;
        --radius: 0.8rem;
      }
    `, "Harbor");
    expect(theme.source).toBe("shadcn");
    expect(theme.radius).toBe("0.8rem");
    expect(theme.roles.card).toBe("#182128");
    expect(theme.roles.brandAccent).toBe("#73c7d3");
  });

  it("imports VS Code workbench colors and adopts a complete ANSI base", () => {
    const theme = importThemeText(`{
      // JSONC is accepted
      "name": "Terminal Night",
      "type": "dark",
      "colors": {
        "editor.background": "#121820",
        "editor.foreground": "#d8e1e8",
        "focusBorder": "#70b8ff",
        "sideBar.background": "#0d131a",
        "terminal.ansiBlack": "#111111",
        "terminal.ansiRed": "#cc5555",
        "terminal.ansiGreen": "#55cc77",
        "terminal.ansiYellow": "#ddbb55",
        "terminal.ansiBlue": "#5599dd",
        "terminal.ansiMagenta": "#aa77dd",
        "terminal.ansiCyan": "#55bbbb",
        "terminal.ansiWhite": "#dddddd",
      },
      "tokenColors": []
    }`);
    expect(theme.source).toBe("vscode");
    expect(theme.roles.sidebar).toBe("#0d131a");
    expect(theme.ansi.black).toBe("#111111");
    expect(theme.ansi.blue).toBe("#5599dd");
    expect(theme.ansi.brightBlue).not.toBe(theme.ansi.blue);
  });

  it("keeps generated ANSI when a VS Code theme provides only a partial base", () => {
    const theme = importThemeText(JSON.stringify({
      name: "Partial ANSI",
      colors: {
        "editor.background": "#121820",
        "focusBorder": "#70b8ff",
        "terminal.ansiBlack": "#000000",
      },
    }));
    expect(theme.ansi.black).not.toBe("#000000");
  });
});

describe("no-flash boot parity", () => {
  const html = readFileSync(resolve(process.cwd(), "index.html"), "utf8");

  it("pins the first-frame fallback to the real default palette", () => {
    expect(html).toContain(`background: "${BUILT_IN_THEMES[0]!.roles.background}"`);
    expect(html).toContain(`foreground: "${BUILT_IN_THEMES[0]!.roles.foreground}"`);
  });

  it("knows every runtime role variable", () => {
    for (const variable of Object.values(THEME_ROLE_VARIABLES)) expect(html).toContain(variable);
  });
});

describe("importThemeDetailed", () => {
  it("names the format and separates what the source gave from what we solved", () => {
    const result = importThemeDetailed(
      ".dark { --background: #11161a; --foreground: #e6edf3; --primary: #e8956a; }",
      "Pasted",
    );
    expect(result.source).toBe("shadcn");
    expect(result.mapped).toEqual(
      expect.arrayContaining(["background", "foreground", "primary"]),
    );
    // Nothing in that block says what a popover or a sidebar looks like.
    expect(result.derived).toEqual(expect.arrayContaining(["popover", "sidebar"]));
    // Every role is accounted for exactly once — the studio prints these counts.
    expect(result.mapped.length + result.derived.length).toBe(THEME_ROLES.length);
    expect(new Set([...result.mapped, ...result.derived]).size).toBe(THEME_ROLES.length);
  });

  it("reports a VS Code theme as such and maps its workbench colors", () => {
    const result = importThemeDetailed(
      JSON.stringify({
        name: "Tokyo Night",
        colors: {
          "editor.background": "#1a1b26",
          "editor.foreground": "#c0caf5",
          "sideBar.background": "#16161e",
          focusBorder: "#7aa2f7",
        },
      }),
      "fallback",
    );
    expect(result.source).toBe("vscode");
    expect(result.theme.label).toBe("Tokyo Night");
    expect(result.mapped).toEqual(
      expect.arrayContaining(["background", "foreground", "sidebar"]),
    );
  });

  it("treats a Codemux file's own roles as mapped, not derived", () => {
    const theme = createGeneratedTheme("Round Trip", "#11161a", "#e8956a");
    const result = importThemeDetailed(serializeTheme(theme));
    expect(result.source).toBe("codemux");
    expect(result.derived).toEqual([]);
    expect(result.mapped).toHaveLength(THEME_ROLES.length);
  });

  it("still throws a showable sentence on junk, since the studio parses every keystroke", () => {
    expect(() => importThemeDetailed("not a theme")).toThrow(/Paste a Codemux theme JSON/);
  });
});

describe("createGeneratedTheme — which input owns which role", () => {
  /** Hue in degrees, or NaN for an achromatic color. */
  function hueOf(value: string): number {
    const raw = (normalizeColor(value) ?? "#000000").replace("#", "");
    const [r, g, b] = [0, 2, 4].map((i) => Number.parseInt(raw.slice(i, i + 2), 16) / 255);
    const max = Math.max(r!, g!, b!);
    const min = Math.min(r!, g!, b!);
    if (max === min) return Number.NaN;
    const d = max - min;
    const h =
      max === r! ? (g! - b!) / d + (g! < b! ? 6 : 0)
      : max === g! ? (b! - r!) / d + 2
      : (r! - g!) / d + 4;
    return (h * 60 + 360) % 360;
  }

  // Regression: every surface used to be derived from the ACCENT's hue, so a
  // blue-black canvas with a warm accent produced a brown app — the
  // background you picked survived only in the role literally named
  // `background`. The two seeds are "the room" and "the highlight".
  const theme = createGeneratedTheme("Night Signal", "#11161a", "#e8956a");

  it("keeps surfaces in the background's hue family", () => {
    for (const role of ["card", "sidebar", "secondary", "border", "input"] as const) {
      expect(Math.abs(hueOf(theme.roles[role]) - hueOf("#11161a"))).toBeLessThan(15);
    }
  });

  it("keeps body text in the background's hue family too", () => {
    expect(Math.abs(hueOf(theme.roles.mutedForeground) - hueOf("#11161a"))).toBeLessThan(20);
  });

  it("leaves the accent roles at the accent's own hue", () => {
    expect(Math.abs(hueOf(theme.roles.brandAccent) - hueOf("#e8956a"))).toBeLessThan(5);
    expect(Math.abs(hueOf(theme.roles.primary) - hueOf("#e8956a"))).toBeLessThan(5);
  });

  it("does not invent saturation a neutral background never had", () => {
    const neutral = createGeneratedTheme("Slate", "#121212", "#e8956a");
    // Achromatic in, achromatic out — hueOf returns NaN for a pure grey.
    expect(Number.isNaN(hueOf(neutral.roles.card))).toBe(true);
    expect(Number.isNaN(hueOf(neutral.roles.sidebar))).toBe(true);
  });

  it("still solves the contrast it promises", () => {
    expect(contrastRatio(theme.roles.foreground, theme.roles.background)).toBeGreaterThanOrEqual(7);
    expect(contrastRatio(theme.roles.mutedForeground, theme.roles.background)).toBeGreaterThanOrEqual(4.5);
  });
});

describe("VS Code import — picking the theme's identity colour", () => {
  // Regression: `focusBorder` was tried FIRST, and real themes very often set
  // it to a translucent grey. Tokyo Night's is `#545c7e33`, which flattened
  // over its canvas to a brown-grey — a mud accent for a theme everyone
  // recognises as blue.
  const tokyoNightish = {
    name: "Tokyo Nightish",
    colors: {
      "editor.background": "#1a1b26",
      "editor.foreground": "#c0caf5",
      focusBorder: "#545c7e33",
      "button.background": "#3d59a1dd",
      "textLink.foreground": "#6183bb",
      "terminal.ansiBlue": "#7aa2f7",
    },
  };

  it("does not let a translucent focusBorder become the brand accent", () => {
    const { theme } = importThemeDetailed(JSON.stringify(tokyoNightish));
    expect(theme.roles.brandAccent.toLowerCase()).not.toBe("#262838");
    expect(contrastRatio(theme.roles.brandAccent, theme.roles.background)).toBeGreaterThanOrEqual(3);
  });

  it("falls through to a bright terminal hue when every brand token is too dark", () => {
    const { theme } = importThemeDetailed(
      JSON.stringify({
        colors: {
          "editor.background": "#101014",
          focusBorder: "#15151a",
          "button.background": "#17171d",
          "terminal.ansiCyan": "#7cdcf0",
        },
      }),
    );
    expect(theme.roles.brandAccent.toLowerCase()).toBe("#7cdcf0");
  });

  it("lifts the best candidate rather than giving up when nothing is legible", () => {
    const { theme } = importThemeDetailed(
      JSON.stringify({
        colors: { "editor.background": "#101014", "activityBarBadge.background": "#16161c" },
      }),
    );
    expect(contrastRatio(theme.roles.brandAccent, theme.roles.background)).toBeGreaterThanOrEqual(2.9);
  });
});
