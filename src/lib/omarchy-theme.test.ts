import { describe, expect, it } from "vitest";
import { initialThemeSource, omarchyToTheme } from "./omarchy-theme";
import { fallbackTheme } from "@/hooks/use-theme-colors";
import { applyTheme, contrastRatio, parseCustomTheme, serializeTheme } from "./themes";

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
    applyTheme(theme, { animate: false });
    expect(document.documentElement.classList.contains("dark")).toBe(scheme === "dark");
    expect(document.documentElement.style.colorScheme).toBe(scheme);
    const copy = parseCustomTheme(JSON.parse(serializeTheme({ ...theme, id: "custom-omarchy-copy" })));
    expect(copy?.scheme).toBe(scheme);
    expect(copy?.roles).toEqual(theme.roles);
  });
});
