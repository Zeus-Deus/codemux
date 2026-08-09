import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => {
  const settings = {
    appearance: {
      theme: "default",
      custom_themes: [] as unknown[],
      shell_font: null,
      terminal_font_size: 13,
      show_resource_monitor: true,
    },
    editor: { default_ide: null },
    terminal: { scrollback_limit: 10_000, cursor_style: "bar" },
    git: { default_base_branch: "main" },
    source_control: { custom_hosts: {} },
    keyboard: { shortcuts: {} },
    notifications: { sound_enabled: true, desktop_enabled: true },
    file_tree: { show_hidden_files: false },
    session_restore: { enabled: true, scrollback_lines: 10_000, max_total_mb: 100 },
    agent_chat: { checkpoints_enabled: false, background_browser_desktop_viewport: true },
    browser: { default_viewport: null },
  };
  return {
    settings,
    updateSetting: vi.fn().mockResolvedValue(undefined),
    updateSettings: vi.fn().mockResolvedValue(undefined),
  };
});

vi.mock("@/stores/synced-settings-store", () => {
  const useSyncedSettingsStore = Object.assign(
    (selector: (state: Record<string, unknown>) => unknown) => selector(mocks),
    { getState: () => mocks },
  );
  return { useSyncedSettingsStore };
});

import { ThemeStudio } from "./theme-studio";
import { applyTheme, BUILT_IN_THEMES, createGeneratedTheme } from "@/lib/themes";
import { useUIStore, type ThemeStudioRequest } from "@/stores/ui-store";

/** The studio is an app-level overlay now: it mounts on a store request
 *  rather than on a settings page, so every test opens it the way a real
 *  entry point does. */
function openStudio(request: ThemeStudioRequest) {
  render(<ThemeStudio />);
  act(() => useUIStore.getState().openThemeStudio(request));
}

describe("ThemeStudio", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    mocks.settings.appearance.theme = "default";
    mocks.settings.appearance.custom_themes = [];
    useUIStore.getState().closeThemeStudio();
    document.documentElement.removeAttribute("data-theme-id");
  });

  it("saves the custom payload and selected id in one atomic settings write", async () => {
    openStudio({ mode: "generate" });

    fireEvent.click(screen.getByRole("button", { name: /Save and apply/i }));

    await waitFor(() => expect(mocks.updateSettings).toHaveBeenCalledTimes(1));
    expect(mocks.updateSetting).not.toHaveBeenCalled();
    const saved = mocks.updateSettings.mock.calls[0]![0];
    expect(saved.appearance.theme).toBe("custom-night-signal");
    expect(saved.appearance.custom_themes).toHaveLength(1);
    expect(saved.appearance.custom_themes[0].id).toBe("custom-night-signal");
    expect(document.documentElement.dataset.themeId).toBe("custom-night-signal");
  });

  it("reopens generated themes from their seeds and preserves their stable id", async () => {
    const theme = createGeneratedTheme("Saved Signal", "#11161a", "#e8956a", "custom-stable-signal");
    mocks.settings.appearance.theme = theme.id;
    mocks.settings.appearance.custom_themes = [theme];
    openStudio({ editThemeId: theme.id });

    // Turn 4: two tabs, Generate and Import — the role editor is a link.
    expect(screen.getByRole("radio", { name: "Generate" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByLabelText("Background color")).toHaveValue("#11161a");
    fireEvent.click(screen.getByRole("button", { name: /Save and apply/i }));

    await waitFor(() => expect(mocks.updateSettings).toHaveBeenCalledTimes(1));
    const saved = mocks.updateSettings.mock.calls[0]![0];
    expect(saved.appearance.theme).toBe("custom-stable-signal");
    expect(saved.appearance.custom_themes).toHaveLength(1);
  });

  it("reopens a seedless theme straight into the role editor — there is nothing else to reopen", () => {
    const theme = {
      ...createGeneratedTheme("Imported", "#121820", "#70b8ff", "custom-imported"),
      source: "json" as const,
      seeds: undefined,
    };
    mocks.settings.appearance.theme = theme.id;
    mocks.settings.appearance.custom_themes = [theme];
    openStudio({ editThemeId: theme.id });

    expect(screen.getByRole("textbox", { name: "Background value" })).toHaveValue("#121820");
    expect(screen.getByRole("textbox", { name: "Bright White value" })).toBeInTheDocument();
    // The tab strip is hidden while the role editor is open — it is a
    // different mode, not a third peer of Generate and Import.
    expect(screen.queryByRole("radio", { name: "Generate" })).toBeNull();
  });
});

describe("ThemeStudio — custom theme lifecycle", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    mocks.settings.appearance.theme = "default";
    mocks.settings.appearance.custom_themes = [];
    useUIStore.getState().closeThemeStudio();
    document.documentElement.removeAttribute("data-theme-id");
  });

  function savedTheme() {
    const theme = createGeneratedTheme("Saved Signal", "#11161a", "#e8956a", "custom-saved");
    mocks.settings.appearance.theme = theme.id;
    mocks.settings.appearance.custom_themes = [theme];
    return theme;
  }

  it("offers export and delete only once the theme exists on disk", () => {
    render(<ThemeStudio />);
    act(() => useUIStore.getState().openThemeStudio({ mode: "generate" }));
    // A draft has nothing to export and nothing to remove.
    expect(screen.queryByRole("button", { name: "Export" })).toBeNull();

    cleanup();
    savedTheme();
    openStudio({ editThemeId: "custom-saved" });
    expect(screen.getByRole("button", { name: "Export" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Delete Saved Signal/ })).toBeInTheDocument();
  });

  it("deleting the applied custom theme drops it and falls back to Graphite", async () => {
    savedTheme();
    openStudio({ editThemeId: "custom-saved" });

    fireEvent.click(screen.getByRole("button", { name: /Delete Saved Signal/ }));

    await waitFor(() => expect(mocks.updateSettings).toHaveBeenCalledTimes(1));
    const saved = mocks.updateSettings.mock.calls[0]![0];
    expect(saved.appearance.custom_themes).toEqual([]);
    expect(saved.appearance.theme).toBe("default");
    // The app must not be left wearing a theme that no longer exists.
    expect(document.documentElement.dataset.themeId).toBe("default");
    expect(useUIStore.getState().themeStudio).toBeNull();
  });
});

describe("ThemeStudio — turn 4 modal", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    mocks.settings.appearance.theme = "default";
    mocks.settings.appearance.custom_themes = [];
    useUIStore.getState().closeThemeStudio();
    document.documentElement.removeAttribute("data-theme-id");
  });

  it("leaves the running app alone — the preview is a panel, not the root", () => {
    applyTheme(BUILT_IN_THEMES[0]!, { animate: false, persist: false });
    openStudio({ mode: "generate" });

    // The old sheet applied the candidate to the real root and reverted on
    // close. Nothing to revert now: the page behind the modal keeps the
    // theme the user actually has.
    expect(document.documentElement.dataset.themeId).toBe("default");
    fireEvent.change(screen.getByLabelText("Background color"), {
      target: { value: "#0d1b2a" },
    });
    expect(document.documentElement.dataset.themeId).toBe("default");
  });

  it("parses on paste — there is no Parse button", async () => {
    openStudio({ mode: "import" });
    // VS Code is reached by name, so its source shows the Marketplace search.
    // The paste box belongs to the two sources you actually paste.
    fireEvent.click(screen.getByRole("radio", { name: "shadcn / Tailwind" }));

    expect(screen.queryByRole("button", { name: /Parse/i })).toBeNull();
    fireEvent.change(screen.getByRole("textbox", { name: "Theme source" }), {
      target: { value: ".dark { --background: #11161a; --primary: #e8956a; }" },
    });

    await waitFor(() =>
      expect(screen.getByText(/Recognised a/)).toBeInTheDocument(),
    );
    expect(screen.getByText("shadcn")).toBeInTheDocument();
    expect(screen.getByText(/roles mapped/)).toBeInTheDocument();
  });

  it("names the roles it had to invent rather than only counting them", async () => {
    openStudio({ mode: "import" });
    fireEvent.click(screen.getByRole("radio", { name: "shadcn / Tailwind" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Theme source" }), {
      target: { value: ".dark { --background: #11161a; --primary: #e8956a; }" },
    });

    await waitFor(() =>
      expect(screen.getByText(/roles? filled in for you/)).toBeInTheDocument(),
    );
    expect(screen.getAllByText("derived").length).toBeGreaterThan(0);
  });

  it("shows a showable error and no candidate when the paste is junk", async () => {
    openStudio({ mode: "import" });
    fireEvent.click(screen.getByRole("radio", { name: "shadcn / Tailwind" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Theme source" }), {
      target: { value: "totally not a theme" },
    });

    await waitFor(() =>
      expect(screen.getByText(/Paste a Codemux theme JSON/)).toBeInTheDocument(),
    );
    expect(screen.getByRole("button", { name: /Save and apply/ })).toBeDisabled();
  });

  it("offers Generate and Import only, with the role editor behind a link", () => {
    openStudio({ mode: "generate" });

    expect(screen.getByRole("radio", { name: "Generate" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Import" })).toBeInTheDocument();
    expect(screen.queryByRole("radio", { name: /Role editor/i })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: /Edit roles by hand/ }));
    expect(screen.getByRole("textbox", { name: "Background value" })).toBeInTheDocument();
  });

  it("swaps the left column for the Marketplace search under the VS Code source", () => {
    openStudio({ mode: "import" });

    // VS Code defaults selected: you find that theme by name, not by file.
    expect(screen.getByRole("radio", { name: "VS Code" })).toHaveAttribute("aria-checked", "true");
    expect(
      screen.getByRole("textbox", { name: "Search the VS Code Marketplace" }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "Theme source" })).toBeNull();

    fireEvent.click(screen.getByRole("radio", { name: "shadcn / Tailwind" }));
    expect(screen.getByRole("textbox", { name: "Theme source" })).toBeInTheDocument();
    expect(
      screen.queryByRole("textbox", { name: "Search the VS Code Marketplace" }),
    ).toBeNull();
  });
});
