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

import { TooltipProvider } from "@/components/ui/tooltip";
import type { AdoptableAgentSession } from "@/tauri/commands";

vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return {
    ...actual,
    listSkills: vi.fn().mockResolvedValue([]),
    startSkillsWatcher: vi.fn().mockResolvedValue(0),
    listChatSlashCommands: vi.fn().mockResolvedValue([]),
    agentChatListAdoptableSessions: vi.fn().mockResolvedValue([]),
  };
});

import { Composer } from "./Composer";
import {
  agentChatListAdoptableSessions,
  listChatSlashCommands,
  listSkills,
} from "@/tauri/commands";
import { useAppStore } from "@/stores/app-store";
import { useProviderCommandsStore } from "@/stores/provider-commands-store";
import { useSkillsStore } from "@/stores/skills-store";

type ComposerProps = ComponentProps<typeof Composer>;

const listAdoptableMock =
  agentChatListAdoptableSessions as unknown as ReturnType<typeof vi.fn>;
const listSkillsMock = listSkills as unknown as ReturnType<typeof vi.fn>;
const listChatSlashCommandsMock =
  listChatSlashCommands as unknown as ReturnType<typeof vi.fn>;

function makeSession(
  overrides: Partial<AdoptableAgentSession> = {},
): AdoptableAgentSession {
  return {
    session_id: "sess-here",
    title: "Refactor the splitter",
    cwd: "/home/user/project",
    git_branch: "main",
    last_modified: new Date().toISOString(),
    created_at: new Date().toISOString(),
    file_size: 4096,
    title_source: "summary",
    existing_thread_id: null,
    same_repo: true,
    project_root: "/home/user/project",
    worktree_name: null,
    ...overrides,
  };
}

/** A session from an unrelated project, as discovery returns it. */
function otherProjectSession(
  overrides: Partial<AdoptableAgentSession> = {},
): AdoptableAgentSession {
  return makeSession({
    session_id: "sess-other",
    title: "Unrelated project",
    cwd: "/projects/ledger",
    project_root: "/projects/ledger",
    same_repo: false,
    ...overrides,
  });
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
    onResumeExternalSession: vi.fn(),
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
    adapterErrors: [],
    loadedAt: 0,
    includePlugins: true,
    inventoryCache: {},
    activeContextKey: null,
    inFlightContexts: {},
    nextRequestId: 1,
    cacheGeneration: 0,
  });
}

/** Type `/`, then click the `/resume` row to open the picker. */
async function openResumePicker(container: HTMLElement) {
  type(getTextarea(container), "/");
  const row = await waitFor(() => {
    const el = document.querySelector('[data-testid="slash-item-composer:resume"]');
    if (!el) throw new Error("no /resume row");
    return el;
  });
  fireEvent.click(row);
  await waitFor(() => {
    expect(
      document.querySelector('[data-testid="composer-command-menu"]'),
    ).not.toBeNull();
  });
}

beforeEach(() => {
  resetSkillsStore();
  useAppStore.setState({ homeDir: "/home/user" });
  useProviderCommandsStore.getState().invalidate();
  listSkillsMock.mockClear();
  listSkillsMock.mockResolvedValue([]);
  listChatSlashCommandsMock.mockClear();
  listChatSlashCommandsMock.mockResolvedValue([]);
  listAdoptableMock.mockClear();
  listAdoptableMock.mockResolvedValue([]);
});

afterEach(() => cleanup());

describe("Composer · /resume command", () => {
  it("offers /resume in the slash popup", () => {
    const { container, queryByTestId } = renderComposer();
    type(getTextarea(container), "/");
    expect(queryByTestId("slash-item-composer:resume")).not.toBeNull();
  });

  it("hides /resume on a surface that cannot adopt", () => {
    const { container, queryByTestId } = renderComposer({
      onResumeExternalSession: undefined,
    });
    type(getTextarea(container), "/");
    expect(queryByTestId("slash-item-composer:resume")).toBeNull();
  });

  it("reserves the name so a provider /resume cannot collide", async () => {
    listChatSlashCommandsMock.mockResolvedValue([
      { name: "resume", description: "Provider resume", argumentHint: "" },
      { name: "compact", description: "Compact the context", argumentHint: "" },
    ]);
    const { container, queryByTestId } = renderComposer();
    type(getTextarea(container), "/");
    await waitFor(() => {
      expect(queryByTestId("slash-item-provider-command:compact")).not.toBeNull();
    });
    expect(queryByTestId("slash-item-provider-command:resume")).toBeNull();
    expect(queryByTestId("slash-item-composer:resume")).not.toBeNull();
  });

  it("strips the typed text instead of inserting it (state-only pick)", async () => {
    const onDraftChange = vi.fn();
    const { container } = renderComposer({ onDraftChange });
    type(getTextarea(container), "/res");
    const row = document.querySelector(
      '[data-testid="slash-item-composer:resume"]',
    )!;
    fireEvent.click(row);
    const calls = onDraftChange.mock.calls;
    expect(calls[calls.length - 1]?.[0]).toBe("");
  });
});

describe("Composer · /resume picker", () => {
  it("discovers every project from the pane's cwd, worktrees included", async () => {
    const { container } = renderComposer();
    await openResumePicker(container);
    await waitFor(() => expect(listAdoptableMock).toHaveBeenCalled());
    const [provider, scope] = listAdoptableMock.mock.calls[0]!;
    expect(provider).toBe("claude");
    expect(scope).toMatchObject({
      current_cwd: "/home/user/project",
      all_projects: true,
      include_worktrees: true,
    });
  });

  it("opens the pane's project and marks the adopted row as a switch", async () => {
    listAdoptableMock.mockResolvedValue([
      makeSession({ session_id: "sess-here", title: "In this checkout" }),
      makeSession({
        session_id: "sess-adopted",
        title: "Already here",
        existing_thread_id: "thread-9",
      }),
      otherProjectSession(),
    ]);
    const { container, findByText, findByTestId, queryByTestId } =
      renderComposer();
    await openResumePicker(container);

    expect(await findByText("In this checkout")).toBeInTheDocument();
    expect(await findByText("Already here")).toBeInTheDocument();
    const project = await findByTestId("resume-folder-/home/user/project");
    expect(project).toHaveAttribute("data-expanded");
    expect(project.textContent).toContain("Selected project");
    expect(
      queryByTestId("slash-item-external-session:sess-adopted")!.textContent,
    ).toContain("already open in Codemux");
    // Other projects are one collapsed line each, opened on demand.
    const ledger = await findByTestId("resume-folder-/projects/ledger");
    expect(ledger).not.toHaveAttribute("data-expanded");
    expect(queryByTestId("slash-item-external-session:sess-other")).toBeNull();
    fireEvent.click(ledger);
    expect(await findByText("Unrelated project")).toBeInTheDocument();
  });

  it("hands the picked session to the pane", async () => {
    const session = makeSession({ session_id: "sess-here" });
    listAdoptableMock.mockResolvedValue([session]);
    const onResumeExternalSession = vi.fn();
    const { container, findByTestId } = renderComposer({
      onResumeExternalSession,
    });
    await openResumePicker(container);

    const row = await findByTestId("slash-item-external-session:sess-here");
    fireEvent.click(row);
    expect(onResumeExternalSession).toHaveBeenCalledWith(session);
    // The picker closes on pick — the pane owns everything after this.
    await waitFor(() => {
      expect(
        document.querySelector('[data-testid="composer-command-menu"]'),
      ).toBeNull();
    });
  });

  it("hands another project's session over without a confirmation step", async () => {
    const session = otherProjectSession();
    listAdoptableMock.mockResolvedValue([makeSession(), session]);
    const onResumeExternalSession = vi.fn();
    const { container, findByTestId } = renderComposer({
      onResumeExternalSession,
    });
    await openResumePicker(container);
    fireEvent.click(await findByTestId("resume-folder-/projects/ledger"));
    fireEvent.click(await findByTestId("slash-item-external-session:sess-other"));
    expect(onResumeExternalSession).toHaveBeenCalledWith(session);
  });

  it("searches from the home directory for a Home draft and leads with RECENT", async () => {
    listAdoptableMock.mockResolvedValue([makeSession(), otherProjectSession()]);
    const { container, findByTestId } = renderComposer({
      cwd: null,
      resumeScope: { cwd: null, projectRoot: null },
    });
    await openResumePicker(container);
    await waitFor(() => expect(listAdoptableMock).toHaveBeenCalled());
    expect(listAdoptableMock.mock.calls[0]![1]).toMatchObject({
      current_cwd: "/home/user",
      all_projects: true,
    });
    expect(await findByTestId("resume-recent")).toBeInTheDocument();
    expect(
      (await findByTestId("resume-folder-/home/user/project")).textContent,
    ).not.toContain("Selected project");
  });

  it("opens the draft's selected project rather than the pane cwd's", async () => {
    listAdoptableMock.mockResolvedValue([makeSession(), otherProjectSession()]);
    const { container, findByTestId } = renderComposer({
      resumeScope: { cwd: "/projects/ledger", projectRoot: "/projects/ledger" },
    });
    await openResumePicker(container);
    expect(
      (await findByTestId("resume-folder-/projects/ledger")),
    ).toHaveAttribute("data-expanded");
    expect(
      await findByTestId("resume-folder-/home/user/project"),
    ).not.toHaveAttribute("data-expanded");
  });

  it("says so when there is no directory to search from", async () => {
    useAppStore.setState({ homeDir: null });
    const { container, findByTestId } = renderComposer({ cwd: null });
    await openResumePicker(container);
    const footer = await findByTestId("slash-popup-footer");
    expect(footer).toHaveAttribute("data-tone", "error");
    expect(footer.textContent).toContain("No working directory to search from");
    expect(listAdoptableMock).not.toHaveBeenCalled();
  });

  it("surfaces a discovery failure in the footer instead of an empty list", async () => {
    listAdoptableMock.mockRejectedValue("sidecar exited");
    const { container, findByTestId } = renderComposer();
    await openResumePicker(container);

    const footer = await findByTestId("slash-popup-footer");
    expect(footer).toHaveAttribute("data-tone", "error");
    expect(footer.textContent).toContain("sidecar exited");
  });
});
