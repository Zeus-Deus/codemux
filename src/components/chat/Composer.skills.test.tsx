/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  waitFor,
  type RenderResult,
} from "@testing-library/react";
import type { ComponentProps } from "react";

import type { Skill } from "@/tauri/commands";
import { TooltipProvider } from "@/components/ui/tooltip";

vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return {
    ...actual,
    listSkills: vi.fn(),
  };
});

import { Composer } from "./Composer";
import { listSkills } from "@/tauri/commands";
import { useSkillsStore } from "@/stores/skills-store";

type ComposerProps = ComponentProps<typeof Composer>;

const listSkillsMock = listSkills as unknown as ReturnType<typeof vi.fn>;

function makeSkill(overrides: Partial<Skill> = {}): Skill {
  return {
    id: "demo-id",
    name: "demo",
    description: "Demo skill",
    provider: "claude",
    scope: "user",
    skillDir: "/skills/demo",
    filePath: "/skills/demo/SKILL.md",
    body: "",
    rawFrontmatter: {},
    bundledFiles: [],
    compatibility: "compatible",
    compatibilitySignals: [],
    symlinked: false,
    pluginSlug: null,
    ...overrides,
  };
}

function baseProps(): ComposerProps {
  return {
    draft: "",
    cwd: "/home/user/project",
    provider: "claude",
    model: null,
    permissionMode: null,
    effort: null,
    contextWindow: null,
    activeModel: null,
    effortLabelMap: {},
    permissionModes: null,
    ultrathinkInBodyText: false,
    streaming: false,
    sessionReady: true,
    showProviderPicker: false,
    mode: "default",
    onDraftChange: vi.fn(),
    onSubmit: vi.fn(),
    onStop: vi.fn(),
    onProviderChange: vi.fn(),
    onModelChange: vi.fn(),
    onPermissionModeChange: vi.fn(),
    onEffortChange: vi.fn(),
    onContextWindowChange: vi.fn(),
    onModeActivate: vi.fn(),
    onModeRemove: vi.fn(),
  };
}

function renderComposer(
  props: Partial<ComposerProps> = {},
): RenderResult {
  return render(
    <TooltipProvider>
      <Composer {...baseProps()} {...props} />
    </TooltipProvider>,
  );
}

function getTextarea(container: HTMLElement) {
  return container.querySelector("textarea") as HTMLTextAreaElement;
}

function type(textarea: HTMLTextAreaElement, value: string, cursor?: number) {
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLTextAreaElement.prototype,
    "value",
  )?.set;
  setter?.call(textarea, value);
  const cur = cursor ?? value.length;
  textarea.setSelectionRange(cur, cur);
  textarea.dispatchEvent(new Event("input", { bubbles: true }));
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

describe("Composer · skills slash integration (Step 7 Stage 2)", () => {
  beforeEach(() => {
    resetSkillsStore();
    listSkillsMock.mockReset();
  });

  afterEach(() => cleanup());

  it("opens the popup, loads skills lazily, and shows them under SKILLS", async () => {
    listSkillsMock.mockResolvedValue([
      makeSkill({ id: "release", name: "codemux-release", description: "Release flow" }),
      makeSkill({ id: "ui", name: "codemux-ui", description: "UI work" }),
    ]);

    const { container, queryByTestId, getByText } = renderComposer({ mode: "default" });

    type(getTextarea(container), "/");

    // Popup opens immediately (modes are synchronous).
    expect(queryByTestId("slash-command-popup")).not.toBeNull();
    // Skill load was kicked off with the workspace cwd.
    expect(listSkillsMock).toHaveBeenCalledWith("/home/user/project", true);

    await waitFor(() => {
      expect(queryByTestId("slash-item-skill:release")).not.toBeNull();
    });
    expect(queryByTestId("slash-item-skill:ui")).not.toBeNull();
    expect(getByText("SKILLS")).toBeInTheDocument();
  });

  it("filters skills as the user types (/codemux-r → only codemux-release)", async () => {
    listSkillsMock.mockResolvedValue([
      makeSkill({ id: "release", name: "codemux-release" }),
      makeSkill({ id: "ui", name: "codemux-ui" }),
    ]);

    const { container, queryByTestId } = renderComposer({ mode: "default" });
    const textarea = getTextarea(container);
    type(textarea, "/");

    await waitFor(() => {
      expect(queryByTestId("slash-item-skill:release")).not.toBeNull();
    });

    type(textarea, "/codemux-r");
    expect(queryByTestId("slash-item-skill:release")).not.toBeNull();
    expect(queryByTestId("slash-item-skill:ui")).toBeNull();
  });

  it("clicking a skill expands the typed query into a literal /skill-name token in the textarea (Cursor-style inline)", async () => {
    listSkillsMock.mockResolvedValue([
      makeSkill({ id: "demo", name: "demo-skill", description: "Stub" }),
    ]);

    const onDraftChange = vi.fn();
    const { container, queryByTestId, getByTestId } = renderComposer({
      mode: "default",
      onDraftChange,
    });
    type(getTextarea(container), "/");

    await waitFor(() => {
      expect(queryByTestId("slash-item-skill:demo")).not.toBeNull();
    });

    fireEvent.click(getByTestId("slash-item-skill:demo"));

    await waitFor(() => {
      expect(onDraftChange).toHaveBeenCalled();
    });
    // Last onDraftChange should be the expansion: "/" → "/demo-skill ".
    const calls = onDraftChange.mock.calls;
    const finalText = calls[calls.length - 1]?.[0];
    expect(finalText).toBe("/demo-skill ");
    expect(queryByTestId("slash-command-popup")).toBeNull();
  });

  it("loading state surfaces a muted footer while the first scan runs", async () => {
    let resolveSkills: (skills: Skill[]) => void = () => {};
    listSkillsMock.mockReturnValue(
      new Promise<Skill[]>((resolve) => {
        resolveSkills = resolve;
      }),
    );

    const { container, queryByTestId, getByTestId } = renderComposer({ mode: "default" });
    type(getTextarea(container), "/");

    await waitFor(() => {
      expect(queryByTestId("slash-popup-footer")).not.toBeNull();
    });
    expect(getByTestId("slash-popup-footer")).toHaveAttribute("data-tone", "muted");
    expect(getByTestId("slash-popup-footer")).toHaveTextContent(/loading skills/i);

    resolveSkills([]);
    await waitFor(() => {
      expect(queryByTestId("slash-popup-footer")).toBeNull();
    });
  });

  it("error state surfaces a destructive footer and keeps modes usable", async () => {
    listSkillsMock.mockRejectedValue(new Error("permission denied"));

    const { container, queryByTestId, getByTestId } = renderComposer({ mode: "default" });
    type(getTextarea(container), "/");

    await waitFor(() => {
      const footer = queryByTestId("slash-popup-footer");
      expect(footer).not.toBeNull();
      expect(footer).toHaveAttribute("data-tone", "error");
    });
    expect(getByTestId("slash-popup-footer")).toHaveTextContent(/permission denied/);

    // Modes still work — popup didn't break for other commands.
    expect(queryByTestId("slash-item-mode:plan")).not.toBeNull();
  });

  it("re-opening the popup within TTL does not re-call listSkills", async () => {
    listSkillsMock.mockResolvedValue([
      makeSkill({ id: "demo", name: "demo-skill" }),
    ]);

    const { container, queryByTestId, rerender } = renderComposer({ mode: "default" });
    const textarea = getTextarea(container);
    type(textarea, "/");
    await waitFor(() => {
      expect(queryByTestId("slash-item-skill:demo")).not.toBeNull();
    });
    expect(listSkillsMock).toHaveBeenCalledTimes(1);

    // Close popup, re-open. Force a textarea event by typing then closing.
    type(textarea, "");
    rerender(
      <TooltipProvider>
        <Composer {...baseProps()} mode="default" />
      </TooltipProvider>,
    );
    type(getTextarea(container), "/");
    await waitFor(() => {
      expect(queryByTestId("slash-item-skill:demo")).not.toBeNull();
    });
    // Cache hit — no extra call.
    expect(listSkillsMock).toHaveBeenCalledTimes(1);
  });

  it("does not load skills when popup never opens", () => {
    listSkillsMock.mockResolvedValue([]);
    renderComposer({ mode: "default" });
    expect(listSkillsMock).not.toHaveBeenCalled();
  });

  it("highlight mirror wraps matched /skill tokens in an amber span", async () => {
    // Seed the skills store directly so the parser has registry data
    // synchronously — bypasses the lazy-load flow which only fires when
    // the slash popup opens.
    useSkillsStore.setState({
      skills: [makeSkill({ id: "demo", name: "omarchy" })],
      loaded: true,
      loading: false,
      error: null,
      loadedAt: Date.now(),
      includePlugins: true,
    });

    const { getByTestId } = renderComposer({
      mode: "default",
      draft: "hi test /omarchy",
    });

    const mirror = getByTestId("composer-highlight-mirror");
    expect(mirror).toHaveTextContent("hi test /omarchy");

    // The matched skill must be wrapped in a status-working-colored span so
    // the user sees it as a syntax-highlighted token. Plain prose around it
    // stays in the default foreground color.
    const highlightSpans = mirror.querySelectorAll("span.text-status-working");
    expect(highlightSpans).toHaveLength(1);
    expect(highlightSpans[0].textContent).toBe("/omarchy");
  });

  it("does NOT highlight an unregistered /token (silent miss, plain prose)", () => {
    useSkillsStore.setState({
      skills: [makeSkill({ id: "demo", name: "omarchy" })],
      loaded: true,
      loading: false,
      error: null,
      loadedAt: Date.now(),
      includePlugins: true,
    });

    const { getByTestId } = renderComposer({
      mode: "default",
      draft: "hi test /notreal",
    });

    const mirror = getByTestId("composer-highlight-mirror");
    expect(mirror.querySelectorAll("span.text-amber-500")).toHaveLength(0);
    expect(mirror).toHaveTextContent("hi test /notreal");
  });

  it("expanding a skill places the cursor right after the inserted token + trailing space", async () => {
    listSkillsMock.mockResolvedValue([
      makeSkill({ id: "demo", name: "omarchy" }),
    ]);

    // Use a mock to prove `onDraftChange` is called with the expansion;
    // also verify the textarea's caret lands at the expected offset.
    let currentDraft = "";
    const onDraftChange = vi.fn((next: string) => {
      currentDraft = next;
    });

    const { container, queryByTestId, getByTestId, rerender } = renderComposer({
      mode: "default",
      onDraftChange,
    });

    type(getTextarea(container), "/");
    await waitFor(() => {
      expect(queryByTestId("slash-item-skill:demo")).not.toBeNull();
    });

    fireEvent.click(getByTestId("slash-item-skill:demo"));

    // The expansion fires onDraftChange before the cursor-positioning
    // RAF fires. Re-render with the new draft so the textarea reflects
    // it (mirrors what the real parent does via state propagation).
    await waitFor(() => {
      expect(currentDraft).toBe("/omarchy ");
    });
    rerender(
      <TooltipProvider>
        <Composer {...baseProps()} mode="default" draft={currentDraft} onDraftChange={onDraftChange} />
      </TooltipProvider>,
    );

    // Wait for the requestAnimationFrame inside handleSlashSelect to
    // run and place the caret. Trailing space length is "/omarchy ".length === 9.
    await waitFor(() => {
      expect(getTextarea(container).selectionStart).toBe("/omarchy ".length);
    });
    expect(getTextarea(container).selectionEnd).toBe("/omarchy ".length);
  });
});
