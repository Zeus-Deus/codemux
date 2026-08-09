/// <reference types="@testing-library/jest-dom/vitest" />
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  settings: {
    appearance: { theme: "default", custom_themes: [] as unknown[] },
  },
}));

vi.mock("@/stores/synced-settings-store", () => {
  const useSyncedSettingsStore = Object.assign(
    (selector: (state: typeof mocks) => unknown) => selector(mocks),
    { getState: () => mocks },
  );
  return { useSyncedSettingsStore };
});

import { ThemeSettings } from "./theme-settings";
import { createGeneratedTheme } from "@/lib/themes";
import { useUIStore } from "@/stores/ui-store";

describe("Appearance theme row", () => {
  afterEach(cleanup);

  beforeEach(() => {
    mocks.settings.appearance.theme = "default";
    mocks.settings.appearance.custom_themes = [];
    useUIStore.setState({
      showCommandPalette: false,
      showSettings: true,
      commandPaletteQuery: null,
      themeStudio: null,
    });
  });

  it("names the applied theme instead of listing every option", () => {
    mocks.settings.appearance.theme = "ember";
    render(<ThemeSettings />);

    expect(screen.getByText("Ember")).toBeInTheDocument();
    expect(screen.queryByText("Graphite")).not.toBeInTheDocument();
    expect(screen.queryByText("Abyss")).not.toBeInTheDocument();
  });

  it("hands Change to the palette, seeded with the theme query", () => {
    render(<ThemeSettings />);
    fireEvent.click(screen.getByRole("button", { name: /Change/ }));

    const ui = useUIStore.getState();
    expect(ui.showCommandPalette).toBe(true);
    expect(ui.commandPaletteQuery).toBe("theme");
    // Settings stays mounted underneath: the picker layers over the page, so
    // Esc — and the studio opened from it — return you to Appearance rather
    // than to the home screen.
    expect(ui.showSettings).toBe(true);
  });

  it("opens the studio on the current theme when it is a custom one", () => {
    const theme = createGeneratedTheme("Night Signal", "#11161a", "#e8956a", "custom-night");
    mocks.settings.appearance.theme = theme.id;
    mocks.settings.appearance.custom_themes = [theme];
    render(<ThemeSettings />);

    fireEvent.click(screen.getByRole("button", { name: "Customize" }));
    expect(useUIStore.getState().themeStudio).toEqual({ editThemeId: "custom-night" });
  });

  it("starts a new theme when the applied one is a built-in it must not edit", () => {
    render(<ThemeSettings />);
    fireEvent.click(screen.getByRole("button", { name: "Customize" }));
    expect(useUIStore.getState().themeStudio).toEqual({ mode: "generate" });
  });
});
