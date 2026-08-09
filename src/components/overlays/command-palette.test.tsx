/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mocks = vi.hoisted(() => ({
  ui: {
    themeStudio: null as unknown,
    openThemeStudio: vi.fn(),
    takeCommandPaletteQuery: vi.fn<() => string | null>(() => null),
    setShowNewWorkspaceDialog: vi.fn(),
  },
  synced: {
    settings: {
      appearance: { theme: "default", custom_themes: [] as unknown[] },
    },
    updateSetting: vi.fn().mockResolvedValue(undefined),
  },
}));

vi.mock("@/stores/ui-store", () => {
  const useUIStore = Object.assign(
    (selector: (state: typeof mocks.ui) => unknown) => selector(mocks.ui),
    { getState: () => mocks.ui },
  );
  return { useUIStore };
});

vi.mock("@/stores/synced-settings-store", () => {
  const useSyncedSettingsStore = Object.assign(
    (selector: (state: typeof mocks.synced) => unknown) => selector(mocks.synced),
    { getState: () => mocks.synced },
  );
  return { useSyncedSettingsStore };
});

vi.mock("@/stores/app-store", () => ({
  useAppStore: (selector: (state: unknown) => unknown) => selector({ appState: null }),
  useHomeDir: () => "/home/z",
  useProjectGroupedWorkspaces: () => [],
}));

vi.mock("@/stores/hosts-store", () => ({ useHosts: () => [] }));

vi.mock("@/stores/sidebar-inbox-store", () => ({
  useSidebarInboxStore: (selector: (state: unknown) => unknown) =>
    selector({ settled: [], snoozed: [], load: async () => {}, setFilter: () => {} }),
}));

vi.mock("@/stores/sidebar-density-store", () => ({
  formatElapsed: () => "1m",
  useSidebarDensityStore: (selector: (state: unknown) => unknown) =>
    selector({ statusSince: {}, settledAt: {} }),
}));

vi.mock("@/components/layout/use-project-appearance", () => ({
  useProjectAppearance: () => ({ customColor: null, imageUrl: null, imageVersion: 0 }),
}));

vi.mock("@/lib/use-coarse-clock", () => ({ useCoarseClock: () => 0 }));
vi.mock("@/hooks/use-resolved-keybinds", () => ({
  useResolvedKeybinds: () => ({ getKeysForAction: () => "" }),
}));
vi.mock("@/hooks/use-keyboard-shortcuts", () => ({ dispatch: vi.fn() }));
vi.mock("@/lib/perf/instrumented-activate", () => ({
  activateWorkspaceInteraction: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@/tauri/commands", () => ({
  createBrowserPane: vi.fn(),
  cyclePane: vi.fn(),
  getPresets: vi.fn().mockResolvedValue({ bar_visible: false }),
  regenerateMcpConfig: vi.fn(),
  setPresetBarVisible: vi.fn(),
}));

import { CommandPalette } from "./command-palette";
import { BUILT_IN_THEMES, applyTheme } from "@/lib/themes";

const GRAPHITE = BUILT_IN_THEMES[0]!;
const EMBER = BUILT_IN_THEMES.find((t) => t.id === "ember")!;

/** The variable the runtime writes — proof the whole app repainted, not
 *  just that a row highlighted. */
const liveThemeId = () => document.documentElement.dataset.themeId;
const liveAccent = () =>
  document.documentElement.style.getPropertyValue("--cm-theme-brand-accent");

function renderPalette() {
  return render(<CommandPalette open onOpenChange={vi.fn()} />);
}

// jsdom implements neither, and cmdk + the list's scroll-to-top effect both
// reach for them on every keystroke.
Element.prototype.scrollTo ??= () => {};
Element.prototype.scrollIntoView ??= () => {};

beforeEach(() => {
  vi.clearAllMocks();
  mocks.synced.settings.appearance.theme = "default";
  mocks.synced.settings.appearance.custom_themes = [];
  mocks.ui.takeCommandPaletteQuery.mockReturnValue(null);
  applyTheme(GRAPHITE, { animate: false, persist: false });
});

afterEach(cleanup);

describe("command palette — theme picker", () => {
  it("keeps themes out of the resting list", () => {
    renderPalette();
    expect(screen.queryByText("Themes")).not.toBeInTheDocument();
    expect(screen.queryByText("Ember")).not.toBeInTheDocument();
  });

  it("surfaces every theme plus the two studio rows for 'theme'", async () => {
    const user = userEvent.setup();
    renderPalette();
    await user.type(screen.getByRole("combobox"), "theme");

    for (const theme of BUILT_IN_THEMES) {
      expect(screen.getByText(theme.label)).toBeInTheDocument();
    }
    expect(screen.getByText("Make a theme from two colors…")).toBeInTheDocument();
    expect(screen.getByText("Paste a VS Code or shadcn theme…")).toBeInTheDocument();
  });

  it("repaints the app for the highlighted row without committing anything", async () => {
    const user = userEvent.setup();
    renderPalette();
    await user.type(screen.getByRole("combobox"), "ember");

    // Highlighting Ember is enough — no click, no Enter.
    await waitFor(() => expect(liveThemeId()).toBe("ember"));
    expect(liveAccent()).toBe(EMBER.roles.brandAccent);
    expect(mocks.synced.updateSetting).not.toHaveBeenCalled();
  });

  it("tags the applied theme as current, whatever is being previewed", async () => {
    const user = userEvent.setup();
    renderPalette();
    await user.type(screen.getByRole("combobox"), "theme");

    // Exactly one row is the real choice — the preview never moves the tag.
    await waitFor(() => expect(screen.getAllByText("current")).toHaveLength(1));
    const graphiteRow = screen.getByText(GRAPHITE.label).closest("[cmdk-item]");
    expect(graphiteRow).toHaveTextContent("current");
  });

  it("swaps the footer to the preview hints while a theme is highlighted", async () => {
    const user = userEvent.setup();
    renderPalette();
    await user.type(screen.getByRole("combobox"), "ember");

    await waitFor(() => expect(screen.getByText("keep it")).toBeInTheDocument());
    expect(screen.getByText("back to Graphite")).toBeInTheDocument();
    expect(screen.getByText("syncs to your account")).toBeInTheDocument();
  });

  it("puts the applied theme back when the highlight leaves the theme list", async () => {
    const user = userEvent.setup();
    renderPalette();
    const input = screen.getByRole("combobox");
    await user.type(input, "ember");
    await waitFor(() => expect(liveThemeId()).toBe("ember"));

    await user.clear(input);
    await user.type(input, ">settings");
    await waitFor(() => expect(liveThemeId()).toBe("default"));
  });

  it("reverts to the applied theme when the palette closes without a choice", async () => {
    const user = userEvent.setup();
    const { unmount } = renderPalette();
    await user.type(screen.getByRole("combobox"), "ember");
    await waitFor(() => expect(liveThemeId()).toBe("ember"));

    // Esc / click-away both unmount the body; that is the single revert path.
    unmount();
    expect(liveThemeId()).toBe("default");
    expect(liveAccent()).toBe(GRAPHITE.roles.brandAccent);
  });

  it("persists to the synced appearance.theme field on Enter", async () => {
    const user = userEvent.setup();
    const onOpenChange = vi.fn();
    render(<CommandPalette open onOpenChange={onOpenChange} />);
    await user.type(screen.getByRole("combobox"), "ember");
    await waitFor(() => expect(liveThemeId()).toBe("ember"));

    await user.keyboard("{Enter}");

    expect(mocks.synced.updateSetting).toHaveBeenCalledWith("appearance", "theme", "ember");
    expect(onOpenChange).toHaveBeenCalledWith(false);
    // The commit must survive the close — the revert path has to stand down.
    cleanup();
    expect(liveThemeId()).toBe("ember");
  });

  it("opens the studio from the create and paste rows", async () => {
    const user = userEvent.setup();
    renderPalette();
    await user.type(screen.getByRole("combobox"), "theme");

    await user.click(screen.getByText("Make a theme from two colors…"));
    expect(mocks.ui.openThemeStudio).toHaveBeenCalledWith({ mode: "generate" });

    cleanup();
    renderPalette();
    await user.type(screen.getByRole("combobox"), "theme");
    await user.click(screen.getByText("Paste a VS Code or shadcn theme…"));
    expect(mocks.ui.openThemeStudio).toHaveBeenCalledWith({ mode: "import" });
  });

  it("opens on the seed query handed to it by Settings' Change button", async () => {
    mocks.ui.takeCommandPaletteQuery.mockReturnValue("theme");
    renderPalette();

    expect(screen.getByRole("combobox")).toHaveValue("theme");
    await waitFor(() => expect(screen.getByText("Ember")).toBeInTheDocument());
  });
});
