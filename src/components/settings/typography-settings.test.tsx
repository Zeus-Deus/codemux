import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  updateSetting: vi.fn(),
  updateSettings: vi.fn(),
}));

vi.mock("@/tauri/commands", () => ({
  getSyncedSettings: vi.fn(),
  resetSyncedSettings: vi.fn(),
  updateSetting: (...args: unknown[]) => mocks.updateSetting(...args),
  updateSyncedSettings: (...args: unknown[]) => mocks.updateSettings(...args),
}));

import { DEFAULT_SETTINGS, useSyncedSettingsStore } from "@/stores/synced-settings-store";
import type { AppearanceSettings } from "@/tauri/types";
import { TypographySettings } from "./typography-settings";

function setAppearance(appearance: Partial<AppearanceSettings>) {
  useSyncedSettingsStore.setState((state) => ({
    settings: {
      ...state.settings,
      appearance: { ...state.settings.appearance, ...appearance },
    },
  }));
}

describe("TypographySettings", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    useSyncedSettingsStore.setState({
      settings: structuredClone(DEFAULT_SETTINGS),
      isLoading: false,
      isSyncing: false,
    });
    mocks.updateSetting.mockImplementation(
      async (section: string, key: string, value: unknown) => ({
        ...structuredClone(DEFAULT_SETTINGS),
        [section]: {
          ...(structuredClone(DEFAULT_SETTINGS) as unknown as Record<string, Record<string, unknown>>)[
            section
          ],
          [key]: value,
        },
      }),
    );
    mocks.updateSettings.mockImplementation(async (settings) => settings);
  });

  it("starts with two linked decisions and truthful live specimens", () => {
    render(<TypographySettings />);

    expect(screen.getByText("Interface & conversation")).toBeInTheDocument();
    expect(screen.getByText("Developer font")).toBeInTheDocument();
    expect(screen.getByText("Conversation · 14px")).toBeInTheDocument();
    expect(screen.getByText("Code · terminal")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Font family: DM Sans" })).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Font family: JetBrains Mono" }),
    ).toBeInTheDocument();
  });

  it("reveals independent surface controls in Advanced mode and persists the mode", async () => {
    render(<TypographySettings />);

    fireEvent.click(screen.getByRole("radio", { name: "Advanced" }));

    await waitFor(() => {
      expect(mocks.updateSetting).toHaveBeenCalledWith(
        "appearance",
        "typography_mode",
        "advanced",
      );
    });
    expect(screen.getByText("Conversation", { selector: "p" })).toBeInTheDocument();
    expect(screen.getByText("Terminal", { selector: "p" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Font family: Follow interface" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Font family: Follow code" })).toBeInTheDocument();
  });

  // A blob migrated from `shell_font` renders that face everywhere; the picker
  // must name it rather than claiming the bundled default is in use.
  it("names the migrated legacy developer font in the picker", () => {
    setAppearance({ shell_font: "Fira Code" });
    render(<TypographySettings />);

    expect(screen.getByRole("button", { name: "Font family: Fira Code" })).toBeInTheDocument();
  });

  // Pre-migration blobs carry no code size and borrow the terminal one, so
  // Advanced mode would otherwise move both surfaces at once.
  it("materializes the code size when Advanced mode makes the surfaces independent", async () => {
    setAppearance({ code_font_size: null, terminal_font_size: 18 });
    render(<TypographySettings />);

    fireEvent.click(screen.getByRole("radio", { name: "Advanced" }));

    await waitFor(() => expect(mocks.updateSettings).toHaveBeenCalledTimes(1));
    expect(mocks.updateSettings.mock.calls[0]![0].appearance).toMatchObject({
      typography_mode: "advanced",
      code_font_size: 18,
      terminal_font_size: 18,
    });
    expect(mocks.updateSetting).not.toHaveBeenCalled();
  });

  it("restores every typography field atomically", async () => {
    setAppearance({
      typography_mode: "advanced",
      interface_font_family: "Atkinson Hyperlegible",
      interface_font_size: 18,
    });
    render(<TypographySettings />);

    fireEvent.click(screen.getByRole("button", { name: "Restore defaults" }));

    await waitFor(() => expect(mocks.updateSettings).toHaveBeenCalledTimes(1));
    const saved = mocks.updateSettings.mock.calls[0]![0];
    expect(saved.appearance).toMatchObject({
      shell_font: null,
      typography_mode: "simple",
      interface_font_family: null,
      interface_font_size: 16,
      conversation_font_family: null,
      conversation_font_size: 14,
      code_font_family: null,
      code_font_size: 13,
      terminal_font_family: null,
      terminal_font_size: 13,
    });
  });
});
