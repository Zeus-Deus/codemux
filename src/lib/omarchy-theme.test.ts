import { describe, expect, it } from "vitest";
import { initialThemeSource, omarchyToTheme } from "./omarchy-theme";
import { fallbackTheme } from "@/hooks/use-theme-colors";
import { applyTheme, contrastRatio, hexToOklch, parseCustomTheme, serializeTheme, themeToSyntaxColors } from "./themes";

it("follows a detected desktop on first launch, but preserves explicit and existing choices", () => {
  const fresh = { available: true, savedSource: undefined, hasPreviousTheme: false, manualThemeId: "default", legacyPalette: undefined };
  expect(initialThemeSource(fresh)).toBe("omarchy");
  expect(initialThemeSource({ ...fresh, available: false })).toBe("manual");
  expect(initialThemeSource({ ...fresh, savedSource: "manual" })).toBe("manual");
  expect(initialThemeSource({ ...fresh, hasPreviousTheme: true })).toBe("manual");
  expect(initialThemeSource({ ...fresh, manualThemeId: "ember" })).toBe("manual");
  expect(initialThemeSource({ ...fresh, legacyPalette: "warm" })).toBe("manual");
  expect(initialThemeSource({ ...fresh, savedSource: "omarchy", available: false })).toBe("omarchy");
});

describe.each(["dark", "light"] as const)("%s Omarchy palette", (scheme) => {
  it("maps the real palette to readable shell roles and keeps terminal ANSI colors", () => {
    const colors = { ...fallbackTheme, background: scheme === "light" ? "#eff1f5" : "#1a1b26", foreground: scheme === "light" ? "#4c4f69" : "#c0caf5", accent: "#1e66f5" };
    const theme = omarchyToTheme({ name: "Tokyo Night", scheme, colors });
    expect(theme.scheme).toBe(scheme);
    expect(theme.roles.background).toBe(colors.background);
    expect(theme.roles.foreground).toBe(colors.foreground);
    expect(theme.ansi.red).toBe(colors.color1);
    expect(contrastRatio(theme.roles.mutedForeground, theme.roles.background)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(theme.roles.primaryForeground, theme.roles.primary)).toBeGreaterThanOrEqual(4.5);
    // Hairlines stay visible on the surface they sit on, whichever way the sidebar recesses.
    expect(contrastRatio(theme.roles.border, theme.roles.background)).toBeGreaterThan(1.1);
    expect(contrastRatio(theme.roles.sidebarBorder, theme.roles.sidebar)).toBeGreaterThan(1.1);
    applyTheme(theme, { animate: false });
    expect(document.documentElement.classList.contains("dark")).toBe(scheme === "dark");
    expect(document.documentElement.style.colorScheme).toBe(scheme);
    const copy = parseCustomTheme(JSON.parse(serializeTheme({ ...theme, id: "custom-omarchy-copy" })));
    expect(copy?.scheme).toBe(scheme);
    expect(copy?.roles).toEqual(theme.roles);
  });
});

it("layers the shell with Omarchy's own shades and derives them when absent", () => {
  const colors = { ...fallbackTheme, background: "#1a1b26", foreground: "#a9b1d6", accent: "#7aa2f7" };
  const surfaces = { dark_background: "#13141c", selection: "#292e42", muted: "#414868" };
  const { roles } = omarchyToTheme({ name: "Tokyo Night", scheme: "dark", colors, surfaces });
  expect(roles.sidebar).toBe(surfaces.dark_background);
  expect(roles.sidebarAccent).toBe(surfaces.selection);
  expect(roles.input).toBe(surfaces.muted);
  // Hairlines stay a quiet step off the canvas, well below Omarchy's muted text shade.
  expect(contrastRatio(roles.border, roles.background)).toBeLessThan(contrastRatio(surfaces.muted, roles.background));
  expect(contrastRatio(roles.sidebarBorder, roles.sidebar)).toBeLessThan(contrastRatio(surfaces.muted, roles.sidebar));
  for (const surface of [roles.background, roles.sidebar]) {
    expect(contrastRatio(roles.mutedForeground, surface)).toBeGreaterThanOrEqual(4.5);
  }
  expect(contrastRatio(roles.sidebarAccentForeground, roles.sidebarAccent)).toBeGreaterThanOrEqual(4.5);
  // Raised surfaces climb T3 Code's ramp off the canvas: card < muted < popover < secondary < accent.
  const lift = (color: string) => contrastRatio(color, roles.background);
  expect(lift(roles.card)).toBeLessThan(lift(roles.muted));
  expect(lift(roles.muted)).toBeLessThan(lift(roles.popover));
  expect(lift(roles.popover)).toBeLessThan(lift(roles.secondary));
  expect(lift(roles.secondary)).toBeLessThan(lift(roles.accent));

  const derived = omarchyToTheme({ name: "Bare", scheme: "dark", colors }).roles;
  expect(derived.sidebar).not.toBe(derived.background);
  expect(contrastRatio(derived.sidebar, "#000000")).toBeLessThan(contrastRatio(derived.background, "#000000"));
});

it("tints raised surfaces with the accent's hue, as T3 Code does", () => {
  // Retro 82: a warm orange accent on a navy canvas gives warm cards, not navy ones.
  const colors = { ...fallbackTheme, background: "#05182e", foreground: "#f6dcac", accent: "#faa968" };
  const { roles } = omarchyToTheme({ name: "Retro 82", scheme: "dark", colors });
  const accentHue = hexToOklch(colors.accent).h;
  for (const surface of [roles.card, roles.popover, roles.secondary, roles.accent]) {
    expect(Math.abs(hexToOklch(surface).h - accentHue)).toBeLessThan(8);
  }
  expect(roles.background).toBe(colors.background);
  expect(roles.foreground).toBe(colors.foreground);

  // A neutral accent has no hue to carry, so surfaces stay in the canvas's family.
  const neutral = omarchyToTheme({ name: "Mono", scheme: "dark", colors: { ...colors, accent: "#8d8d8d" } }).roles;
  expect(Math.abs(hexToOklch(neutral.card).h - hexToOklch(colors.background).h)).toBeLessThan(8);
});

it("keeps Omarchy's own terminal cursor and selection", () => {
  const colors = { ...fallbackTheme, cursor: "#f6dcac", selection_foreground: "#f6dcac", selection_background: "#134e5a", color8: "#2a6b78" };
  const terminal = themeToSyntaxColors(omarchyToTheme({ name: "Retro 82", scheme: "dark", colors }));
  expect(terminal.cursor).toBe(colors.cursor);
  expect(terminal.selection_foreground).toBe(colors.selection_foreground);
  expect(terminal.selection_background).toBe(colors.selection_background);
});

it("caps a loud Omarchy muted shade on control outlines", () => {
  // Vantablack's muted is #7a7a7a on pure black: fine for text, glaring as an outline.
  const colors = { ...fallbackTheme, background: "#000000", foreground: "#ffffff", accent: "#8d8d8d" };
  const surfaces = { dark_background: "#090909", selection: "#1a1a1a", muted: "#7a7a7a" };
  const { roles } = omarchyToTheme({ name: "Vantablack", scheme: "dark", colors, surfaces });
  expect(contrastRatio(roles.input, roles.background)).toBeLessThanOrEqual(2.5);
  expect(contrastRatio(roles.input, roles.background)).toBeGreaterThan(contrastRatio(roles.border, roles.background));
});
