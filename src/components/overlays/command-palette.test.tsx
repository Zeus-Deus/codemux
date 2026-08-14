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
  app: {
    appState: null as unknown,
  },
  backend: {
    agentChatSearch: vi.fn(),
    openConversationSearchResult: vi.fn(),
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
  useAppStore: (selector: (state: unknown) => unknown) => selector(mocks.app),
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
  agentChatSearch: mocks.backend.agentChatSearch,
  createBrowserPane: vi.fn(),
  cyclePane: vi.fn(),
  getPresets: vi.fn().mockResolvedValue({ bar_visible: false }),
  regenerateMcpConfig: vi.fn(),
  setPresetBarVisible: vi.fn(),
}));

vi.mock("@/lib/agent-chat/conversation-search", () => ({
  openConversationSearchResult: mocks.backend.openConversationSearchResult,
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
  mocks.app.appState = null;
  mocks.backend.agentChatSearch.mockResolvedValue([]);
  mocks.backend.openConversationSearchResult.mockResolvedValue(undefined);
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

  it("keeps the seeded Change flow anchored on Themes through crowded results", async () => {
    const workspaces = Array.from({ length: 30 }, (_, index) => ({
      workspace_id: `ws-theme-${index}`,
      title: `Theme investigation ${index}`,
      cwd: `/repo-${index}`,
      git_branch: `feature/theme-investigation-${index}`,
      project_root: `/repo-${index}`,
      surfaces: [],
      active_surface_id: "",
    }));
    mocks.app.appState = {
      active_workspace_id: "ws-theme-0",
      pane_statuses: {},
      workspaces,
    };
    mocks.backend.agentChatSearch.mockResolvedValue([
      {
        message_id: 91,
        thread_id: "thread-theme",
        workspace_id: "ws-theme-0",
        cwd: "/repo-0",
        provider: "codex",
        session_title: "Theme notes from an older conversation",
        role: "assistant",
        turn_id: "turn-theme",
        snippet: "The theme keyword also appears in this conversation.",
        created_at: "2026-08-14 00:00:00",
      },
    ]);
    mocks.ui.takeCommandPaletteQuery.mockReturnValue("theme");
    renderPalette();

    expect(screen.getByRole("combobox")).toHaveValue("theme");
    expect(screen.getByText("Workspaces")).toBeInTheDocument();
    expect(screen.getByText("24 of 30")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByText("Ember")).toBeInTheDocument());
    const themesHeader = document.querySelector('[data-palette-group="Themes"]');
    const workspacesHeader = document.querySelector('[data-palette-group="Workspaces"]');
    expect(themesHeader?.compareDocumentPosition(workspacesHeader!)).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );

    // Conversation hits arrive later, but the deep link keeps Themes first
    // instead of allowing that asynchronous result to shift the target away.
    const conversation = await screen.findByText("Theme notes from an older conversation");
    expect(themesHeader?.compareDocumentPosition(conversation)).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
  });
});

describe("command palette — conversation search", () => {
  it("searches open workspaces and opens a durable transcript hit", async () => {
    const hit = {
      message_id: 42,
      thread_id: "thread-42",
      workspace_id: "ws-1",
      cwd: "/repo",
      provider: "claude",
      session_title: "Virtualized transcript fix",
      role: "assistant",
      turn_id: "turn-7",
      snippet: "The virtualized transcript now preserves its anchor.",
      created_at: "2026-08-10 10:00:00",
    } as const;
    mocks.app.appState = {
      active_workspace_id: "ws-1",
      pane_statuses: {},
      workspaces: [
        {
          workspace_id: "ws-1",
          title: "Search branch",
          cwd: "/repo",
          git_branch: "feature/search",
          project_root: "/repo",
          surfaces: [],
          active_surface_id: "",
        },
      ],
    };
    mocks.backend.agentChatSearch.mockResolvedValue([hit]);
    const user = userEvent.setup();

    renderPalette();
    await user.type(screen.getByRole("combobox"), "virtualized");

    await waitFor(() =>
      expect(mocks.backend.agentChatSearch).toHaveBeenCalledWith(
        "virtualized",
        ["ws-1"],
        12,
      ),
    );
    expect(await screen.findByText("Conversations")).toBeInTheDocument();
    expect(screen.getByText("Virtualized transcript fix")).toBeInTheDocument();
    expect(screen.getByText("Agent")).toBeInTheDocument();

    await user.click(screen.getByText("Virtualized transcript fix"));
    expect(mocks.backend.openConversationSearchResult).toHaveBeenCalledWith(hit);
  });

  it("survives app-state emits that only change workspace identities", async () => {
    const hit = {
      message_id: 7,
      thread_id: "thread-7",
      workspace_id: "ws-1",
      cwd: "/repo",
      provider: "claude",
      session_title: "Anchor preserved",
      role: "assistant",
      turn_id: "turn-1",
      snippet: "The anchor is preserved.",
      created_at: "2026-08-10 10:00:00",
    } as const;
    // A fresh state object with fresh workspace objects — what every pane
    // status or streaming emit produces, even when nothing relevant moved.
    const stateWith = (ids: string[]) => ({
      active_workspace_id: ids[0] ?? "",
      pane_statuses: {},
      workspaces: ids.map((id) => ({
        workspace_id: id,
        title: `Workspace ${id}`,
        cwd: "/repo",
        git_branch: "feature/search",
        project_root: "/repo",
        surfaces: [],
        active_surface_id: "",
      })),
    });
    mocks.app.appState = stateWith(["ws-1"]);
    mocks.backend.agentChatSearch.mockResolvedValue([hit]);
    const user = userEvent.setup();

    const { rerender } = renderPalette();
    await user.type(screen.getByRole("combobox"), "anchor");
    expect(await screen.findByText("Anchor preserved")).toBeInTheDocument();
    expect(mocks.backend.agentChatSearch).toHaveBeenCalledTimes(1);

    mocks.app.appState = stateWith(["ws-1"]);
    rerender(<CommandPalette open onOpenChange={vi.fn()} />);

    // Rows stay put, and nothing re-queries once the debounce window passes.
    expect(screen.getByText("Anchor preserved")).toBeInTheDocument();
    await new Promise((resolve) => setTimeout(resolve, 250));
    expect(mocks.backend.agentChatSearch).toHaveBeenCalledTimes(1);
    expect(screen.getByText("Anchor preserved")).toBeInTheDocument();

    // A real change to the open set is still worth a fresh query.
    mocks.app.appState = stateWith(["ws-1", "ws-2"]);
    rerender(<CommandPalette open onOpenChange={vi.fn()} />);
    await waitFor(() =>
      expect(mocks.backend.agentChatSearch).toHaveBeenLastCalledWith(
        "anchor",
        ["ws-1", "ws-2"],
        12,
      ),
    );
    expect(mocks.backend.agentChatSearch).toHaveBeenCalledTimes(2);
  });
});
