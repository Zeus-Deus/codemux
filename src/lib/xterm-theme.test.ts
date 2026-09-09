import { describe, expect, it } from "vitest";

import {
  BUILT_IN_THEMES,
  contrastRatio,
  themeToSyntaxColors,
  type ThemeDefinition,
} from "./themes";
import { themeColorsToXtermTheme } from "./xterm-theme";

const themeFor = (id: string): ThemeDefinition =>
  BUILT_IN_THEMES.find((theme) => theme.id === id)!;

const xtermFor = (id: string) =>
  themeColorsToXtermTheme(themeToSyntaxColors(themeFor(id)));

describe("terminal selection", () => {
  // The named ANSI slots keep their literal meaning on both canvases —
  // `black` is dark and `white` is light even on a white terminal — so the
  // dark scheme's pairing (`white` on `brightBlack`) is only ~3:1 once the
  // palette flips. Each scheme has to take its own pair.
  it.each(["default", "graphite-light"])("stays legible on %s", (id) => {
    const theme = xtermFor(id);
    expect(
      contrastRatio(theme.selectionForeground!, theme.selectionBackground!),
    ).toBeGreaterThanOrEqual(4.5);
  });

  it.each(["default", "graphite-light"])("stays visible against the canvas on %s", (id) => {
    const theme = xtermFor(id);
    // A selection the user can't find is as broken as one they can't read.
    expect(
      contrastRatio(theme.selectionBackground!, theme.background!),
    ).toBeGreaterThan(1.15);
  });

  it("puts the light scheme's ink on the light scheme's fill", () => {
    const light = themeFor("graphite-light");
    const theme = xtermFor("graphite-light");
    expect(theme.selectionForeground).toBe(light.ansi.black);
    expect(theme.selectionBackground).toBe(light.ansi.white);
  });
});

describe("themeColorsToXtermTheme", () => {
  it("reads the cursor's ink off the canvas so a block cursor inverts", () => {
    for (const id of ["default", "graphite-light"]) {
      const theme = xtermFor(id);
      expect(theme.cursorAccent).toBe(theme.background);
      expect(contrastRatio(theme.cursor!, theme.background!)).toBeGreaterThan(2);
    }
  });
});
