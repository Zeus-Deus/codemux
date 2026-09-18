/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, waitFor, type RenderResult } from "@testing-library/react";
import type { ComponentProps } from "react";

import type { Skill } from "@/tauri/commands";
import { TooltipProvider } from "@/components/ui/tooltip";

vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return {
    ...actual,
    listSkills: vi.fn(),
    startSkillsWatcher: vi.fn().mockResolvedValue(0),
    listChatSlashCommands: vi.fn(),
  };
});

import { Composer } from "./Composer";
import { listChatSlashCommands, listSkills } from "@/tauri/commands";
import { useProviderCommandsStore } from "@/stores/provider-commands-store";
import { useSkillsStore } from "@/stores/skills-store";

type ComposerProps = ComponentProps<typeof Composer>;

const listSkillsMock = listSkills as unknown as ReturnType<typeof vi.fn>;
const listChatSlashCommandsMock =
  listChatSlashCommands as unknown as ReturnType<typeof vi.fn>;

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
    onProviderModelChange: vi.fn(),
    onModelChange: vi.fn(),
    onPermissionModeChange: vi.fn(),
    onEffortChange: vi.fn(),
    onContextWindowChange: vi.fn(),
    onModeActivate: vi.fn(),
    onModeRemove: vi.fn(),
  };
}

function renderComposer(props: Partial<ComposerProps> = {}): RenderResult {
  return render(
    <TooltipProvider>
      <Composer {...baseProps()} {...props} />
    </TooltipProvider>,
  );
}

function resetSkillsStore() {
  useSkillsStore.setState({
    skills: [],
    loaded: false,
    loading: false,
    error: null,
    adapterErrors: [],
    loadedAt: 0,
    includePlugins: true,
    disabledIds: [],
    inventoryCache: {},
    activeContextKey: null,
    inFlightContexts: {},
    nextRequestId: 1,
    cacheGeneration: 0,
  });
}

/**
 * A draft that opens with `/goal` runs something the moment it is sent,
 * and before this the composer painted it as ordinary prose. These
 * cover the two affordances that say otherwise: the chip above the
 * textarea and the accented token inside it.
 */
describe("Composer · command execution affordance", () => {
  beforeEach(() => {
    resetSkillsStore();
    useProviderCommandsStore.getState().invalidate();
    listSkillsMock.mockReset();
    listSkillsMock.mockResolvedValue([]);
    listChatSlashCommandsMock.mockReset();
    listChatSlashCommandsMock.mockResolvedValue([
      {
        name: "goal",
        description: "Set a standing goal for this thread",
        argumentHint: "<goal text>",
      },
    ]);
  });

  afterEach(() => cleanup());

  it("discovers commands for a slash-leading draft without the popup ever opening", async () => {
    // The user's own report came from a pasted `/goal <url>` — no popup
    // was opened, so the old popup-gated discovery never ran and the
    // command stayed invisible right up to send.
    const { findByTestId } = renderComposer({
      draft: "/goal https://example.com/issues/370",
    });

    const chip = await findByTestId("composer-command-chip");
    expect(chip).toHaveAttribute("data-command-kind", "provider");
    expect(chip).toHaveTextContent("Runs /goal");
    expect(chip).toHaveTextContent("Set a standing goal for this thread");
  });

  it("highlights the leading command token inside the mirror", async () => {
    const { findByTestId, getByTestId } = renderComposer({
      draft: "/goal https://example.com/issues/370",
    });

    const token = await findByTestId("composer-command-token-goal");
    expect(token).toHaveTextContent("/goal");
    // The mirror still renders every original character — it is the
    // layer the transparent textarea's caret is positioned against.
    expect(getByTestId("composer-highlight-mirror")).toHaveTextContent(
      "/goal https://example.com/issues/370",
    );
  });

  it("shows the argument hint only while the command is still bare", async () => {
    const bare = renderComposer({ draft: "/goal" });
    expect(await bare.findByTestId("composer-command-chip")).toHaveTextContent(
      "<goal text>",
    );
    cleanup();

    const filled = renderComposer({ draft: "/goal ship the release" });
    expect(
      await filled.findByTestId("composer-command-chip"),
    ).not.toHaveTextContent("<goal text>");
  });

  it("labels a leading skill as a skill run", async () => {
    listSkillsMock.mockResolvedValue([
      makeSkill({ id: "rel", name: "codemux-release", description: "Release flow" }),
    ]);

    const { findByTestId } = renderComposer({
      draft: "/codemux-release cut 0.22.9",
    });

    const chip = await findByTestId("composer-command-chip");
    expect(chip).toHaveAttribute("data-command-kind", "skill");
    expect(chip).toHaveTextContent("Runs /codemux-release");
  });

  it.each(["claude", "codex", "cursor", "grok", "opencode"] as const)(
    "announces a skill run on the %s provider, not just Claude",
    async (provider) => {
      // Skills are projected to every chat provider, so the affordance
      // has to be provider-agnostic. Provider-native commands are a
      // narrower story — only adapters with a discovery surface report
      // any — but nothing here may be gated on the adapter.
      listSkillsMock.mockResolvedValue([
        makeSkill({ id: "rel", name: "codemux-release" }),
      ]);

      const { findByTestId } = renderComposer({
        provider,
        draft: "/codemux-release cut 0.22.9",
      });

      expect(await findByTestId("composer-command-chip")).toHaveAttribute(
        "data-command-kind",
        "skill",
      );
      await findByTestId("composer-command-token-codemux-release");
    },
  );

  it("keeps a provider command whose name only collides with qualified skill tokens", async () => {
    // Two same-named skills are addressed by qualified tokens, so the
    // bare name is offered by nobody. Reserving it for the skills would
    // strand the provider's own command: no chip, no highlight, and a
    // command that still runs on send.
    listSkillsMock.mockResolvedValue([
      makeSkill({ id: "a", name: "goal", provider: "claude", scope: "project" }),
      makeSkill({ id: "b", name: "goal", provider: "claude", scope: "user" }),
    ]);

    const { findByTestId } = renderComposer({ draft: "/goal ship the release" });

    const chip = await findByTestId("composer-command-chip");
    expect(chip).toHaveAttribute("data-command-kind", "provider");
    expect(chip).toHaveTextContent("Runs /goal");
  });

  it("still lets a uniquely-named skill claim its bare name over a provider command", async () => {
    listSkillsMock.mockResolvedValue([makeSkill({ id: "a", name: "goal" })]);

    const { findByTestId } = renderComposer({ draft: "/goal ship the release" });

    expect(await findByTestId("composer-command-chip")).toHaveAttribute(
      "data-command-kind",
      "skill",
    );
  });

  it("picks up a session-fed catalogue that was empty on the first read", async () => {
    // A session-fed provider answers with nothing until its session has
    // published. That first empty answer must not stick, or the real
    // catalogue stays hidden for the rest of the app's lifetime.
    listChatSlashCommandsMock
      .mockResolvedValueOnce([])
      .mockResolvedValue([
        { name: "plan-feature", description: "Draft a plan (project)" },
      ]);

    const view = renderComposer({
      provider: "cursor",
      draft: "/plan-feature add offline mode",
    });
    await waitFor(() =>
      expect(listChatSlashCommandsMock).toHaveBeenCalledTimes(1),
    );
    expect(view.queryByTestId("composer-command-chip")).toBeNull();

    // Leaving command context and coming back is what re-reads it.
    view.rerender(
      <TooltipProvider>
        <Composer {...baseProps()} provider="cursor" draft="" />
      </TooltipProvider>,
    );
    view.rerender(
      <TooltipProvider>
        <Composer
          {...baseProps()}
          provider="cursor"
          draft="/plan-feature add offline mode"
        />
      </TooltipProvider>,
    );

    expect(await view.findByTestId("composer-command-chip")).toHaveTextContent(
      "Runs /plan-feature",
    );
  });

  it("keeps a static catalogue memoised instead of re-reading it", async () => {
    const view = renderComposer({ draft: "/goal ship it" });
    await view.findByTestId("composer-command-chip");

    view.rerender(
      <TooltipProvider>
        <Composer {...baseProps()} draft="" />
      </TooltipProvider>,
    );
    view.rerender(
      <TooltipProvider>
        <Composer {...baseProps()} draft="/goal ship it" />
      </TooltipProvider>,
    );
    await view.findByTestId("composer-command-chip");

    // Claude's catalogue comes from a probe that spawns a child process,
    // so re-entering command context must not repeat it.
    expect(listChatSlashCommandsMock).toHaveBeenCalledTimes(1);
  });

  it("stays quiet for prose, an unknown token, and a pasted absolute path", async () => {
    // Prime discovery first so these assertions can't pass merely
    // because the registry hadn't arrived yet.
    const primed = renderComposer({ draft: "/goal ship it" });
    await primed.findByTestId("composer-command-chip");
    cleanup();

    for (const draft of [
      "just fix the bug",
      "/notacommand do the thing",
      "/home/user/project/README.md is the input",
      "please run /goal later",
    ]) {
      const { queryByTestId } = renderComposer({ draft });
      await waitFor(() =>
        expect(queryByTestId("composer-command-chip")).toBeNull(),
      );
      cleanup();
    }
  });
});
