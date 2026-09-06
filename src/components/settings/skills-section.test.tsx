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
import { useSkillsStore } from "@/stores/skills-store";
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

  it("renders skills grouped by scope · provider", async () => {
    listSkillsMock.mockResolvedValue([
      makeSkill({ id: "u-c", name: "user-claude", provider: "claude", scope: "user" }),
      makeSkill({ id: "u-x", name: "user-codex", provider: "codex", scope: "user" }),
      makeSkill({
        id: "p",
        name: "plugin-skill",
        provider: "claude",
        scope: "plugin",
        pluginSlug: "demo",
      }),
    ]);
    render(<SkillsSection projectRoot={null} />);
    await waitFor(() => {
      expect(screen.getByText("User · Claude")).toBeInTheDocument();
    });
    expect(screen.getByText("User · Codex")).toBeInTheDocument();
    expect(screen.getByText("Plugin")).toBeInTheDocument();
    expect(screen.getByText("user-claude")).toBeInTheDocument();
    expect(screen.getByText("user-codex")).toBeInTheDocument();
    expect(screen.getByText("plugin-skill")).toBeInTheDocument();
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

  it("renders a 'Naming conflicts' section at the top when same-name skills exist", async () => {
    listSkillsMock.mockResolvedValue([
      makeSkill({
        id: "u-release",
        name: "release",
        provider: "claude",
        scope: "user",
        description: "User-wide release",
      }),
      makeSkill({
        id: "p-release",
        name: "release",
        provider: "claude",
        scope: "project",
        description: "Project release",
      }),
      makeSkill({ id: "alone", name: "lonely", provider: "claude", scope: "user" }),
    ]);

    render(<SkillsSection projectRoot={null} />);

    await waitFor(() => {
      expect(screen.getByTestId("skills-conflicts")).toBeInTheDocument();
    });
    const conflictGroup = screen.getByTestId("conflict-group-release");
    expect(conflictGroup).toHaveTextContent("release");
    expect(conflictGroup).toHaveTextContent("User · Claude");
    expect(conflictGroup).toHaveTextContent("Project · Claude");

    // The unique-named skill stays in its normal scope group, not in
    // the conflict block.
    expect(
      screen.queryByTestId("conflict-group-lonely"),
    ).not.toBeInTheDocument();
    expect(screen.getByText("lonely")).toBeInTheDocument();
  });

  it("toggling a per-row switch flips disabled state in the store", async () => {
    listSkillsMock.mockResolvedValue([
      makeSkill({ id: "demo", name: "demo-skill" }),
    ]);
    render(<SkillsSection projectRoot={null} />);
    await waitFor(() => {
      expect(screen.getByText("demo-skill")).toBeInTheDocument();
    });

    expect(useSkillsStore.getState().disabledIds).toEqual([]);
    fireEvent.click(screen.getByTestId("skill-row-switch-demo"));

    await waitFor(() => {
      expect(useSkillsStore.getState().disabledIds).toEqual(["demo"]);
    });
    // Row stays rendered, just visually disabled.
    expect(screen.getByText("demo-skill")).toBeInTheDocument();
    expect(screen.getByTestId("skill-row-disabled-badge")).toBeInTheDocument();
  });

  it("does NOT render the conflicts section when every name is unique", async () => {
    listSkillsMock.mockResolvedValue([
      makeSkill({ id: "a", name: "alpha", provider: "claude", scope: "user" }),
      makeSkill({ id: "b", name: "beta", provider: "codex", scope: "user" }),
    ]);
    render(<SkillsSection projectRoot={null} />);
    await waitFor(() => {
      expect(screen.getByText("alpha")).toBeInTheDocument();
    });
    expect(screen.queryByTestId("skills-conflicts")).not.toBeInTheDocument();
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
