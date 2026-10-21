/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";

import type { Skill } from "@/tauri/commands";

vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return {
    ...actual,
    listSkills: vi.fn(),
    detectEditors: vi.fn(),
    openInEditor: vi.fn(),
    startSkillsWatcher: vi.fn().mockResolvedValue(0),
    stopSkillsWatcher: vi.fn().mockResolvedValue(undefined),
  };
});

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock("@/lib/toast", () => ({
  toast: {
    info: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
    error: vi.fn(),
    dismiss: vi.fn(),
    custom: vi.fn(),
  },
}));

import { SkillsSection } from "./skills-section";
import {
  detectEditors,
  listSkills,
  openInEditor,
  startSkillsWatcher,
} from "@/tauri/commands";
import { _resetEditorDiscoveryForTests } from "@/stores/editor-discovery-store";
import { listen } from "@tauri-apps/api/event";
import { toast } from "@/lib/toast";
import { selectActiveSkills, useSkillsStore } from "@/stores/skills-store";
import {
  DEFAULT_SETTINGS,
  useSyncedSettingsStore,
} from "@/stores/synced-settings-store";

const listSkillsMock = listSkills as unknown as ReturnType<typeof vi.fn>;
const detectEditorsMock = detectEditors as unknown as ReturnType<typeof vi.fn>;
const openInEditorMock = openInEditor as unknown as ReturnType<typeof vi.fn>;

function makeSkill(overrides: Partial<Skill> = {}): Skill {
  return {
    id: `id-${overrides.name ?? "demo"}`,
    name: "demo",
    description: "Demo desc",
    provider: "claude",
    scope: "user",
    skillDir: "/skills/demo",
    filePath: "/skills/demo/SKILL.md",
    body: "Body.",
    rawFrontmatter: {},
    bundledFiles: [],
    compatibility: "compatible",
    compatibilitySignals: [],
    symlinked: false,
    pluginSlug: null,
    ...overrides,
  };
}

function resetSkillsStore() {
  useSkillsStore.setState({
    skills: [],
    loaded: false,
    loading: false,
    error: null,
    loadedAt: 0,
    includePlugins: true,
    disabledIds: [],
  });
}

function setPreferredEditor(editorId: string) {
  useSyncedSettingsStore.setState({
    settings: {
      ...DEFAULT_SETTINGS,
      editor: { ...DEFAULT_SETTINGS.editor, default_ide: editorId },
    },
    isLoading: false,
    isSyncing: false,
  });
}

function clearPreferredEditor() {
  useSyncedSettingsStore.setState({
    settings: DEFAULT_SETTINGS,
    isLoading: false,
    isSyncing: false,
  });
}

describe("SkillsSection", () => {
  beforeEach(() => {
    _resetEditorDiscoveryForTests();
    resetSkillsStore();
    clearPreferredEditor();
    listSkillsMock.mockReset();
    detectEditorsMock.mockReset();
    openInEditorMock.mockReset();
    (toast.error as ReturnType<typeof vi.fn>).mockReset();
  });

  afterEach(() => cleanup());

  it("calls loadSkills with force=true on mount", async () => {
    listSkillsMock.mockResolvedValue([]);
    render(<SkillsSection projectRoot="/proj" />);
    await waitFor(() => {
      expect(listSkillsMock).toHaveBeenCalledWith("/proj", true, true);
    });
  });

  it("shows the loading state during the initial fetch", () => {
    let resolveSkills: (s: Skill[]) => void = () => {};
    listSkillsMock.mockReturnValue(
      new Promise<Skill[]>((resolve) => {
        resolveSkills = resolve;
      }),
    );
    render(<SkillsSection projectRoot={null} />);
    expect(screen.getByTestId("skills-loading")).toBeInTheDocument();
    resolveSkills([]);
  });

  it("shows the empty state when no skills are returned", async () => {
    listSkillsMock.mockResolvedValue([]);
    render(<SkillsSection projectRoot={null} />);
    await waitFor(() => {
      expect(screen.getByTestId("skills-empty")).toBeInTheDocument();
    });
  });

  it("shows the error state when the fetch rejects", async () => {
    listSkillsMock.mockRejectedValue(new Error("permission denied"));
    render(<SkillsSection projectRoot={null} />);
    await waitFor(() => {
      const err = screen.getByTestId("skills-error");
      expect(err).toHaveTextContent(/permission denied/i);
    });
  });

  it("renders one row per skill, tagged with every source", async () => {
    listSkillsMock.mockResolvedValue([
      makeSkill({ id: "shared", name: "shared", provider: "claude" }),
      makeSkill({ id: "shared", name: "shared", provider: "codex" }),
      makeSkill({
        id: "p",
        name: "plugin-skill",
        provider: "claude",
        scope: "plugin",
        pluginSlug: "demo",
        filePath: "/plugins/demo/SKILL.md",
      }),
    ]);
    render(<SkillsSection projectRoot={null} />);
    await waitFor(() => {
      expect(screen.getByText("shared")).toBeInTheDocument();
    });
    expect(screen.getAllByText("shared")).toHaveLength(1);
    const sharedRow = screen.getByTestId("skill-row-shared");
    expect(sharedRow).toHaveTextContent("Claude");
    expect(sharedRow).toHaveTextContent("Codex");
    expect(screen.getByTestId("skill-row-p")).toHaveTextContent("Plugin");
    expect(
      screen.queryByTestId("skills-differing-note"),
    ).not.toBeInTheDocument();
  });

  it("filters rows by text and by source", async () => {
    listSkillsMock.mockResolvedValue([
      makeSkill({ id: "a", name: "alpha", provider: "claude", filePath: "/a" }),
      makeSkill({ id: "b", name: "beta", provider: "codex", filePath: "/b" }),
    ]);
    render(<SkillsSection projectRoot={null} />);
    await waitFor(() => {
      expect(screen.getByText("alpha")).toBeInTheDocument();
    });

    fireEvent.change(screen.getByLabelText("Filter skills"), {
      target: { value: "bet" },
    });
    expect(screen.queryByText("alpha")).not.toBeInTheDocument();
    expect(screen.getByText("beta")).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Filter skills"), {
      target: { value: "" },
    });
    fireEvent.click(screen.getByRole("radio", { name: "Claude" }));
    expect(screen.getByText("alpha")).toBeInTheDocument();
    expect(screen.queryByText("beta")).not.toBeInTheDocument();
  });

  it("Refresh button forces a re-fetch even when within TTL", async () => {
    listSkillsMock.mockResolvedValue([]);
    render(<SkillsSection projectRoot={null} />);
    await waitFor(() => {
      expect(listSkillsMock).toHaveBeenCalledTimes(1);
    });

    fireEvent.click(screen.getByRole("button", { name: /refresh skills/i }));
    await waitFor(() => {
      expect(listSkillsMock).toHaveBeenCalledTimes(2);
    });
  });

  it("toggling 'Include plugin-bundled skills' invalidates cache and re-fetches with the new flag", async () => {
    listSkillsMock.mockResolvedValue([]);
    render(<SkillsSection projectRoot={null} />);
    await waitFor(() => {
      expect(listSkillsMock).toHaveBeenCalledWith(null, true, true);
    });

    fireEvent.click(screen.getByTestId("include-plugins-switch"));
    await waitFor(() => {
      expect(listSkillsMock).toHaveBeenLastCalledWith(null, false, true);
    });
  });

  it("clicking View on a skill row opens the modal with that skill", async () => {
    listSkillsMock.mockResolvedValue([
      makeSkill({ id: "demo", name: "demo-skill", description: "yo" }),
    ]);
    render(<SkillsSection projectRoot={null} />);
    await waitFor(() => {
      expect(screen.getByText("demo-skill")).toBeInTheDocument();
    });

    fireEvent.click(screen.getByRole("button", { name: /view/i }));
    await waitFor(() => {
      expect(screen.getByTestId("skill-view-modal")).toBeInTheDocument();
    });
  });

  it("clicking Open file calls detectEditors then openInEditor with the file path", async () => {
    listSkillsMock.mockResolvedValue([
      makeSkill({
        id: "demo",
        name: "demo-skill",
        filePath: "/home/user/.claude/skills/demo/SKILL.md",
      }),
    ]);
    detectEditorsMock.mockResolvedValue([
      { id: "code", name: "VS Code", binary: "code" },
    ]);
    openInEditorMock.mockResolvedValue(undefined);

    render(<SkillsSection projectRoot={null} />);
    await waitFor(() => {
      expect(screen.getByText("demo-skill")).toBeInTheDocument();
    });

    fireEvent.click(
      screen.getByRole("button", { name: /open demo-skill in editor/i }),
    );
    await waitFor(() => {
      expect(openInEditorMock).toHaveBeenCalledWith(
        "code",
        "/home/user/.claude/skills/demo/SKILL.md",
      );
    });
  });

  it("Open file honors the user's configured default IDE when set", async () => {
    setPreferredEditor("cursor");
    listSkillsMock.mockResolvedValue([
      makeSkill({
        id: "demo",
        name: "demo-skill",
        filePath: "/skills/demo/SKILL.md",
      }),
    ]);
    detectEditorsMock.mockResolvedValue([
      { id: "code", name: "VS Code", binary: "code" },
      { id: "cursor", name: "Cursor", binary: "cursor" },
    ]);
    openInEditorMock.mockResolvedValue(undefined);

    render(<SkillsSection projectRoot={null} />);
    await waitFor(() => {
      expect(screen.getByText("demo-skill")).toBeInTheDocument();
    });

    fireEvent.click(
      screen.getByRole("button", { name: /open demo-skill in editor/i }),
    );
    await waitFor(() => {
      // User picked Cursor in their settings — even though VS Code is
      // the first detected, the preference wins.
      expect(openInEditorMock).toHaveBeenCalledWith(
        "cursor",
        "/skills/demo/SKILL.md",
      );
    });
  });

  it("falls back to the first detected editor when the configured default is no longer installed", async () => {
    setPreferredEditor("uninstalled-editor");
    listSkillsMock.mockResolvedValue([
      makeSkill({ id: "demo", name: "demo-skill" }),
    ]);
    detectEditorsMock.mockResolvedValue([
      { id: "code", name: "VS Code", binary: "code" },
    ]);
    openInEditorMock.mockResolvedValue(undefined);

    render(<SkillsSection projectRoot={null} />);
    await waitFor(() => {
      expect(screen.getByText("demo-skill")).toBeInTheDocument();
    });

    fireEvent.click(
      screen.getByRole("button", { name: /open demo-skill in editor/i }),
    );
    await waitFor(() => {
      expect(openInEditorMock).toHaveBeenCalledWith(
        "code",
        "/skills/demo/SKILL.md",
      );
    });
  });

  it("flags same-named skills whose content differs and lists their versions", async () => {
    listSkillsMock.mockResolvedValue([
      makeSkill({
        id: "fresh",
        name: "release",
        provider: "claude",
        filePath: "/fleet/release/SKILL.md",
        body: "New steps.",
      }),
      makeSkill({
        id: "fresh",
        name: "release",
        provider: "codex",
        filePath: "/fleet/release/SKILL.md",
        body: "New steps.",
      }),
      makeSkill({
        id: "stale",
        name: "release",
        provider: "codemux",
        filePath: "/codemux/release/SKILL.md",
        body: "Old steps.",
      }),
    ]);

    render(<SkillsSection projectRoot={null} />);

    await waitFor(() => {
      expect(screen.getByTestId("skills-differing-note")).toHaveTextContent(
        "1 skill has different versions",
      );
    });
    expect(screen.getAllByText("release")).toHaveLength(1);
    const toggle = screen.getByRole("button", { name: /2 versions/i });
    expect(toggle).toHaveAttribute("aria-expanded", "false");

    fireEvent.click(toggle);
    const versions = screen.getByTestId("skill-versions-release");
    expect(versions).toHaveTextContent("/claude:user:release");
    expect(versions).toHaveTextContent("/codemux:user:release");
    expect(versions).toHaveTextContent("/codemux/release/SKILL.md");

    // Each version keeps its own switch.
    fireEvent.click(
      screen.getByRole("switch", {
        name: "Remove /codemux:user:release from Codemux",
      }),
    );
    expect(useSkillsStore.getState().disabledIds).toEqual(["stale"]);

    // The fresh version is still offered, so the row still reads as on.
    expect(screen.getByTestId("skill-row-switch-fresh")).toBeChecked();
    expect(
      screen.queryByTestId("skill-row-disabled-badge"),
    ).not.toBeInTheDocument();
    expect(
      selectActiveSkills(useSkillsStore.getState()).map((s) => s.id),
    ).toEqual(["fresh", "fresh"]);

    // Switching the row off then disables every version.
    fireEvent.click(screen.getByTestId("skill-row-switch-fresh"));
    expect(useSkillsStore.getState().disabledIds).toEqual(["fresh", "stale"]);
    expect(screen.getByTestId("skill-row-disabled-badge")).toBeInTheDocument();
  });

  it("falls back to All when the selected source leaves the inventory", async () => {
    const claude = makeSkill({ id: "c", name: "claude-skill", filePath: "/c" });
    const plugin = makeSkill({
      id: "p",
      name: "plugin-skill",
      scope: "plugin",
      pluginSlug: "demo",
      filePath: "/p",
    });
    listSkillsMock.mockResolvedValue([claude, plugin]);
    render(<SkillsSection projectRoot={null} />);
    await waitFor(() => {
      expect(screen.getByText("plugin-skill")).toBeInTheDocument();
    });
    fireEvent.click(screen.getByRole("radio", { name: "Plugin" }));
    expect(screen.queryByText("claude-skill")).not.toBeInTheDocument();

    // Excluding plugin skills reloads an inventory without that source.
    listSkillsMock.mockResolvedValue([claude]);
    fireEvent.click(screen.getByTestId("include-plugins-switch"));
    await waitFor(() => {
      expect(screen.getByText("claude-skill")).toBeInTheDocument();
    });
    expect(screen.queryByTestId("skills-no-matches")).not.toBeInTheDocument();
  });

  it("a row's switch disables every copy of the skill together", async () => {
    listSkillsMock.mockResolvedValue([
      makeSkill({ id: "demo", name: "demo-skill", body: "Same." }),
      makeSkill({
        id: "demo-copy",
        name: "demo-skill",
        provider: "codemux",
        filePath: "/codemux/demo/SKILL.md",
        body: "Same.",
      }),
    ]);
    render(<SkillsSection projectRoot={null} />);
    await waitFor(() => {
      expect(screen.getByText("demo-skill")).toBeInTheDocument();
    });

    expect(useSkillsStore.getState().disabledIds).toEqual([]);
    fireEvent.click(screen.getByTestId("skill-row-switch-demo"));

    await waitFor(() => {
      expect(useSkillsStore.getState().disabledIds).toEqual([
        "demo",
        "demo-copy",
      ]);
    });
    // Row stays rendered, just visually disabled.
    expect(screen.getByText("demo-skill")).toBeInTheDocument();
    expect(screen.getByTestId("skill-row-disabled-badge")).toBeInTheDocument();
  });

  it("starts the file watcher on mount", async () => {
    listSkillsMock.mockResolvedValue([]);
    render(<SkillsSection projectRoot="/proj" />);
    await waitFor(() => {
      expect(startSkillsWatcher).toHaveBeenCalledWith("/proj", true);
    });
  });

  it("re-loads skills when a 'skills-changed' event fires", async () => {
    let capturedHandler: ((e: { payload: unknown }) => void) | null = null;
    (listen as ReturnType<typeof vi.fn>).mockImplementation(
      (_event: string, handler: (e: { payload: unknown }) => void) => {
        capturedHandler = handler;
        return Promise.resolve(() => {});
      },
    );
    listSkillsMock.mockResolvedValue([]);

    render(<SkillsSection projectRoot={null} />);
    await waitFor(() => {
      expect(capturedHandler).not.toBeNull();
    });

    // Initial mount load + the includePlugins-triggered load happen on
    // first render. Reset so we can isolate the watcher-driven refresh.
    const callsBefore = listSkillsMock.mock.calls.length;

    capturedHandler!({ payload: undefined });
    await waitFor(() => {
      expect(listSkillsMock.mock.calls.length).toBeGreaterThan(callsBefore);
    });
  });

  it("toasts an error when no editor is detected", async () => {
    listSkillsMock.mockResolvedValue([
      makeSkill({ id: "demo", name: "demo-skill" }),
    ]);
    detectEditorsMock.mockResolvedValue([]);

    render(<SkillsSection projectRoot={null} />);
    await waitFor(() => {
      expect(screen.getByText("demo-skill")).toBeInTheDocument();
    });

    fireEvent.click(
      screen.getByRole("button", { name: /open demo-skill in editor/i }),
    );
    await waitFor(() => {
      expect(toast.error).toHaveBeenCalledWith(
        "No editor detected",
        expect.any(Object),
      );
    });
  });
});
