/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  waitFor,
  type RenderResult,
} from "@testing-library/react";
import { useState, type ComponentProps } from "react";

import { TooltipProvider } from "@/components/ui/tooltip";
import type { FolderMatch, FileMatch } from "@/tauri/types";

vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return {
    ...actual,
    listProjectFiles: vi.fn().mockResolvedValue([]),
    listProjectFolders: vi.fn().mockResolvedValue([]),
    listGithubIssuesByPath: vi.fn().mockResolvedValue([]),
    agentChatListSessionMentions: vi.fn().mockResolvedValue([]),
    getGithubIssueByPath: vi.fn(),
    listSkills: vi.fn().mockResolvedValue([]),
  };
});

import { Composer } from "./Composer";
import {
  agentChatListSessionMentions,
  getGithubIssueByPath,
  listGithubIssuesByPath,
  listProjectFiles,
  listProjectFolders,
} from "@/tauri/commands";
import type { AgentChatSessionMention } from "@/tauri/commands";
import type { GitHubIssue } from "@/tauri/types";

type ComposerProps = ComponentProps<typeof Composer>;

const listProjectFilesMock = listProjectFiles as unknown as ReturnType<typeof vi.fn>;
const listProjectFoldersMock = listProjectFolders as unknown as ReturnType<typeof vi.fn>;
const listGithubIssuesMock =
  listGithubIssuesByPath as unknown as ReturnType<typeof vi.fn>;
const getGithubIssueMock =
  getGithubIssueByPath as unknown as ReturnType<typeof vi.fn>;
const listSessionMentionsMock =
  agentChatListSessionMentions as unknown as ReturnType<typeof vi.fn>;

function makeSession(
  overrides: Partial<AgentChatSessionMention> = {},
): AgentChatSessionMention {
  return {
    thread_id: "thread-aaa111",
    workspace_id: "workspace-1",
    cwd: "/repo",
    provider: "codex",
    title: "Harden authentication",
    last_active_at: new Date().toISOString(),
    preview: "Implemented refresh token rotation.",
    message_count: 12,
    ...overrides,
  };
}

function makeIssue(overrides: Partial<GitHubIssue> = {}): GitHubIssue {
  return {
    number: 1234,
    title: "Login redirect bug",
    state: "Open",
    labels: ["bug"],
    assignees: [],
    url: "https://github.com/u/r/issues/1234",
    body: null,
    comments: [],
    totalComments: 0,
    updatedAt: "2026-04-20T00:00:00Z",
    ...overrides,
  };
}

function makeFile(overrides: Partial<FileMatch> = {}): FileMatch {
  return {
    path: "src/components/chat/Composer.tsx",
    absolute_path: "/repo/src/components/chat/Composer.tsx",
    score: 0,
    ...overrides,
  };
}

function makeFolder(overrides: Partial<FolderMatch> = {}): FolderMatch {
  return {
    path: "src/components/chat",
    absolute_path: "/repo/src/components/chat",
    score: 0,
    item_count: 5,
    ...overrides,
  };
}

function baseProps(): ComposerProps {
  return {
    draft: "",
    cwd: "/repo",
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

function ControlledComposer(props: Partial<ComposerProps> = {}) {
  const [draft, setDraft] = useState(props.draft ?? "");
  const baseline = baseProps();
  return (
    <Composer
      {...baseline}
      {...props}
      draft={draft}
      onDraftChange={(next) => {
        setDraft(next);
        props.onDraftChange?.(next);
      }}
    />
  );
}

function renderControlled(
  props: Partial<ComposerProps> = {},
): RenderResult {
  return render(
    <TooltipProvider>
      <ControlledComposer {...props} />
    </TooltipProvider>,
  );
}

beforeEach(() => {
  listProjectFilesMock.mockReset();
  listProjectFoldersMock.mockReset();
  listGithubIssuesMock.mockReset();
  getGithubIssueMock.mockReset();
  listProjectFilesMock.mockResolvedValue([]);
  listProjectFoldersMock.mockResolvedValue([]);
  listGithubIssuesMock.mockResolvedValue([]);
});

afterEach(() => cleanup());

describe("Composer + button + attach popup (Step 8 Stage 3)", () => {
  it("the + button is always visible in the footer (locked decision: persistent)", () => {
    const { getByTestId } = renderControlled();
    expect(getByTestId("composer-attach-button")).toBeInTheDocument();
  });

  it("clicking the + button opens the popup with MODES + ATTACH groups", () => {
    const { getByTestId, getByText, queryByText } = renderControlled();
    fireEvent.click(getByTestId("composer-attach-button"));
    // MODES at the top — primary affordance now that the `+ Mode`
    // dropdown is gone.
    expect(getByText("Plan")).toBeInTheDocument();
    expect(getByText("Debug")).toBeInTheDocument();
    expect(getByText("Ask")).toBeInTheDocument();
    // ATTACH below.
    expect(getByText("File…")).toBeInTheDocument();
    expect(getByText("Folder…")).toBeInTheDocument();
    expect(getByText("GitHub Issue…")).toBeInTheDocument();
    // NAVIGATION group dropped — `/` and `@` keyboard paths stay
    // self-evident; the MODES section makes them discoverable.
    expect(queryByText("Slash commands")).toBeNull();
    expect(queryByText("Mention")).toBeNull();
  });

  it("picking Plan from the + popup activates Plan mode + closes the popup", () => {
    const onModeActivate = vi.fn();
    const { getByTestId, queryByText } = renderControlled({
      onModeActivate,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-mode:plan"));
    expect(onModeActivate).toHaveBeenCalledWith("plan");
    expect(queryByText("Plan")).toBeNull();
  });

  it("picking Debug from the + popup activates Debug mode", () => {
    const onModeActivate = vi.fn();
    const { getByTestId } = renderControlled({ onModeActivate });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-mode:debug"));
    expect(onModeActivate).toHaveBeenCalledWith("debug");
  });

  it("picking Ask from the + popup activates Ask mode", () => {
    const onModeActivate = vi.fn();
    const { getByTestId } = renderControlled({ onModeActivate });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-mode:ask"));
    expect(onModeActivate).toHaveBeenCalledWith("ask");
  });

  it("the currently-active mode renders disabled in the + popup (no double-activate)", () => {
    const onModeActivate = vi.fn();
    const { getByTestId } = renderControlled({
      mode: "plan",
      onModeActivate,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    const plan = getByTestId("slash-item-mode:plan");
    expect(plan.getAttribute("data-disabled")).toBe("true");
    fireEvent.click(plan);
    expect(onModeActivate).not.toHaveBeenCalled();
  });

  it("disabled coming-soon entries render with the disabled attribute", () => {
    const { getByTestId } = renderControlled();
    fireEvent.click(getByTestId("composer-attach-button"));
    const issueRow = getByTestId("slash-item-attach:issue");
    const prRow = getByTestId("slash-item-attach:pr");
    const imageRow = getByTestId("slash-item-attach:image");
    expect(issueRow.getAttribute("data-disabled")).toBe("true");
    expect(prRow.getAttribute("data-disabled")).toBe("true");
    expect(imageRow.getAttribute("data-disabled")).toBe("true");
    // Redesign: disabled rows dim to ~0.42 (design contract) rather
    // than the legacy 0.5.
    expect(issueRow.className).toContain("opacity-[0.42]");
  });

  it("clicking a disabled entry is a no-op (popup stays on main)", async () => {
    const { getByTestId, getByText } = renderControlled();
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:issue"));
    // Still on main — File… still visible.
    expect(getByText("File…")).toBeInTheDocument();
  });

  it("shows /workflow enabled for the Claude provider in the + popup", () => {
    const { getByTestId, getByText } = renderControlled({ provider: "claude" });
    fireEvent.click(getByTestId("composer-attach-button"));
    const row = getByTestId("slash-item-workflow");
    // cmdk emits `data-disabled="false"` for enabled items; either a
    // missing attribute or "false" both mean "enabled".
    const attr = row.getAttribute("data-disabled");
    expect(attr === null || attr === "false").toBe(true);
    expect(getByText("Orchestrate this task with many subagents")).toBeInTheDocument();
  });

  it("picking /workflow from the + popup inserts the literal /workflow token", () => {
    const onDraftChange = vi.fn();
    const { getByTestId } = renderControlled({
      provider: "claude",
      onDraftChange,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-workflow"));
    const calls = onDraftChange.mock.calls;
    const finalText = calls[calls.length - 1]?.[0];
    expect(finalText).toBe("/workflow ");
  });

  it.each(["codex", "opencode"] as const)(
    "shows /workflow disabled with a reason for the %s provider, and picking it is a no-op",
    (provider) => {
      const onDraftChange = vi.fn();
      const { getByTestId, getByText } = renderControlled({
        provider,
        onDraftChange,
      });
      fireEvent.click(getByTestId("composer-attach-button"));
      const row = getByTestId("slash-item-workflow");
      expect(row.getAttribute("data-disabled")).toBe("true");
      expect(getByText("Only available with Claude models")).toBeInTheDocument();
      fireEvent.click(row);
      expect(onDraftChange).not.toHaveBeenCalled();
    },
  );

  it("picking File… pivots to the file submode and fetches files", async () => {
    listProjectFilesMock.mockResolvedValue([makeFile()]);
    const { getByTestId, findByText } = renderControlled({
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:file"));
    expect(
      await findByText("src/components/chat/Composer.tsx"),
    ).toBeInTheDocument();
    expect(listProjectFilesMock).toHaveBeenCalledWith("/repo", null, 30);
  });

  it("picking Folder… pivots to the folder submode and fetches folders", async () => {
    listProjectFoldersMock.mockResolvedValue([makeFolder()]);
    const { getByTestId, findByText } = renderControlled({
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:folder"));
    expect(
      await findByText("src/components/chat"),
    ).toBeInTheDocument();
    expect(listProjectFoldersMock).toHaveBeenCalledWith("/repo", null, 30);
  });

  it("picking a folder inserts the inline @<basename> token + calls onAttachFolder", async () => {
    listProjectFoldersMock.mockResolvedValue([makeFolder()]);
    const onAttachFolder = vi.fn();
    const onDraftChange = vi.fn();
    const { getByTestId, findByText } = renderControlled({
      onAttachFolder,
      onDraftChange,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:folder"));
    const row = await findByText("src/components/chat");
    fireEvent.click(row);
    expect(onAttachFolder).toHaveBeenCalledWith(makeFolder());
    const calls = onDraftChange.mock.calls;
    const finalText = calls[calls.length - 1]?.[0];
    expect(finalText).toBe("@chat ");
  });

  it("picking a file from the + → File… view inserts the inline @<basename> token", async () => {
    listProjectFilesMock.mockResolvedValue([makeFile()]);
    const onAttachFile = vi.fn();
    const onDraftChange = vi.fn();
    const { getByTestId, findByText } = renderControlled({
      onAttachFile,
      onDraftChange,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:file"));
    const row = await findByText("src/components/chat/Composer.tsx");
    fireEvent.click(row);
    expect(onAttachFile).toHaveBeenCalledWith(makeFile());
    const calls = onDraftChange.mock.calls;
    const finalText = calls[calls.length - 1]?.[0];
    expect(finalText).toBe("@Composer.tsx ");
  });

  it("Esc inside a submode walks back to main", async () => {
    listProjectFoldersMock.mockResolvedValue([makeFolder()]);
    const { getByTestId, findByText, getByText, queryByText } =
      renderControlled();
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:folder"));
    await findByText("src/components/chat");
    // The redesigned command menu owns a focused search input; Escape
    // is handled there (not on the textarea) and bubbles to the menu's
    // Escape handler.
    fireEvent.keyDown(getByTestId("composer-command-search"), {
      key: "Escape",
    });
    // Back on main: File… visible, folder rows gone.
    expect(getByText("File…")).toBeInTheDocument();
    expect(queryByText("src/components/chat")).toBeNull();
  });

  it("Esc on main closes the popup entirely", () => {
    const { getByTestId, queryByText } = renderControlled({
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.keyDown(getByTestId("composer-command-search"), {
      key: "Escape",
    });
    expect(queryByText("File…")).toBeNull();
  });

  it("clicking the button while open toggles the popup closed", () => {
    const { getByTestId, queryByText } = renderControlled({
    });
    const btn = getByTestId("composer-attach-button");
    fireEvent.click(btn);
    expect(queryByText("File…")).not.toBeNull();
    fireEvent.click(btn);
    expect(queryByText("File…")).toBeNull();
  });

  it("opening the attach popup closes any open mention popup", async () => {
    listProjectFilesMock.mockResolvedValue([makeFile()]);
    const { getByTestId, findByText, queryByText } = renderControlled({
    });
    // Open mention popup by typing @
    const textarea = document.querySelector(
      'textarea',
    ) as HTMLTextAreaElement;
    fireEvent.change(textarea, {
      target: {
        value: "@composer",
        selectionStart: 9,
        selectionEnd: 9,
      },
    });
    await findByText("src/components/chat/Composer.tsx");
    // Now click the + button — mention popup closes, attach popup opens
    fireEvent.click(getByTestId("composer-attach-button"));
    // The mention row should no longer be visible (the file submode
    // hasn't been picked yet so no row should show).
    expect(queryByText("src/components/chat/Composer.tsx")).toBeNull();
    expect(queryByText("File…")).toBeInTheDocument();
  });

  it("renders the loading footer note while folders are being fetched", async () => {
    let resolve!: (m: FolderMatch[]) => void;
    listProjectFoldersMock.mockReturnValue(
      new Promise<FolderMatch[]>((r) => {
        resolve = r;
      }),
    );
    const { getByTestId } = renderControlled();
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:folder"));
    await waitFor(() => {
      const footer = getByTestId("slash-popup-footer");
      expect(footer.textContent).toContain("Loading folders");
    });
    resolve([]);
  });

  it("shows the cwd-null hint when no project is anchored and a submode is opened", () => {
    const { getByTestId } = renderControlled({
      cwd: null,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:file"));
    const footer = getByTestId("slash-popup-footer");
    expect(footer.textContent).toContain("project");
  });
});

describe("Composer command menu — search + structure (redesign)", () => {
  it("renders the anchored command menu surface with a search box", () => {
    const { getByTestId } = renderControlled();
    fireEvent.click(getByTestId("composer-attach-button"));
    expect(getByTestId("composer-command-menu")).toBeInTheDocument();
    expect(getByTestId("composer-command-search")).toBeInTheDocument();
  });

  it("surfaces the MCP entry under its own INTEGRATIONS group", () => {
    const { getByTestId, getByText } = renderControlled();
    fireEvent.click(getByTestId("composer-attach-button"));
    expect(getByText("INTEGRATIONS")).toBeInTheDocument();
    expect(getByTestId("slash-item-attach:mcp")).toBeInTheDocument();
  });

  it("filters rows across groups as the user types in the search box", () => {
    const { getByTestId, queryByTestId } = renderControlled();
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.change(getByTestId("composer-command-search"), {
      target: { value: "debug" },
    });
    // Only the Debug mode row survives — other modes + attach rows are
    // filtered out across every group.
    expect(queryByTestId("slash-item-mode:debug")).not.toBeNull();
    expect(queryByTestId("slash-item-mode:plan")).toBeNull();
    expect(queryByTestId("slash-item-attach:file")).toBeNull();
  });

  it("matches a leading / against the mode tags (/pl → Plan)", () => {
    const { getByTestId, queryByTestId } = renderControlled();
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.change(getByTestId("composer-command-search"), {
      target: { value: "/pl" },
    });
    expect(queryByTestId("slash-item-mode:plan")).not.toBeNull();
    expect(queryByTestId("slash-item-mode:ask")).toBeNull();
    // Attach rows have no tag, so the slash-scoped match excludes them.
    expect(queryByTestId("slash-item-attach:file")).toBeNull();
  });

  it("shows the empty state when the query matches nothing", () => {
    const { getByTestId, getByText } = renderControlled();
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.change(getByTestId("composer-command-search"), {
      target: { value: "zzzznope" },
    });
    expect(getByText(/No matches/i)).toBeInTheDocument();
  });

  it("keeps disabled rows VISIBLE with their reason (not hidden) + dimmed", () => {
    // Contract: outside a GitHub repo the Issue / PR rows must NOT
    // disappear — they render at reduced opacity, non-selectable, with
    // the reason swapped into the description slot.
    const { getByTestId } = renderControlled({ repoSupported: false });
    fireEvent.click(getByTestId("composer-attach-button"));
    const issue = getByTestId("slash-item-attach:issue");
    expect(issue).toBeInTheDocument();
    expect(issue.getAttribute("data-disabled")).toBe("true");
    expect(issue.textContent).toContain("Not a GitHub repo");
    expect(issue.className).toContain("opacity-[0.42]");
    expect(issue.className).toContain("cursor-not-allowed");
  });

  it("Enter in the search box selects the first enabled row (cmdk nav)", () => {
    // The menu's focused cmdk input owns keyboard nav; the first
    // enabled row (Plan) is auto-highlighted, so Enter activates it.
    const onModeActivate = vi.fn();
    const { getByTestId } = renderControlled({ onModeActivate });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.keyDown(getByTestId("composer-command-search"), {
      key: "Enter",
    });
    expect(onModeActivate).toHaveBeenCalledWith("plan");
  });
});

describe("Composer + popup → GitHub Issue submode (Step 8 Stage 4)", () => {
  it("disables the GitHub Issue row when repoSupported is false", () => {
    const { getByTestId } = renderControlled({ repoSupported: false });
    fireEvent.click(getByTestId("composer-attach-button"));
    const row = getByTestId("slash-item-attach:issue");
    expect(row.getAttribute("data-disabled")).toBe("true");
    expect(row.textContent).toContain("Not a GitHub repo");
  });

  it("disables the GitHub Issue row when gh is not authenticated", () => {
    const { getByTestId } = renderControlled({
      repoSupported: true,
      providerAuthenticated: false,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    const row = getByTestId("slash-item-attach:issue");
    expect(row.getAttribute("data-disabled")).toBe("true");
    expect(row.textContent).toContain("gh auth login");
  });

  // Regression: a missing CLI used to leave the hosting rows ENABLED —
  // the preflight reported "not installed" as "the auth question does
  // not apply", which the gate read as "nothing known to be wrong". The
  // row opened a picker that could only error. A CLI that is not on
  // PATH cannot serve a picker, so the rows gate on it too, and the
  // copy names the download rather than a login for a binary that isn't
  // there.
  it("disables the hosting rows when the provider CLI is not installed", () => {
    for (const providerAuthenticated of [false, null] as const) {
      cleanup();
      const { getByTestId } = renderControlled({
        repoSupported: true,
        providerCliInstalled: false,
        providerAuthenticated,
      });
      fireEvent.click(getByTestId("composer-attach-button"));
      for (const id of ["attach:issue", "attach:pr"]) {
        const row = getByTestId(`slash-item-${id}`);
        expect(row.getAttribute("data-disabled")).toBe("true");
        expect(row.textContent).toContain("Install gh from cli.github.com");
        expect(row.textContent).not.toContain("gh auth login");
      }
    }
  });

  it("names the checkout's own CLI in the not-installed hint", () => {
    const { getByTestId } = renderControlled({
      repoSupported: true,
      providerKind: "gitlab",
      providerCliInstalled: false,
      providerAuthenticated: false,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    const row = getByTestId("slash-item-attach:pr");
    expect(row.getAttribute("data-disabled")).toBe("true");
    expect(row.textContent).toContain(
      "Install glab from gitlab.com/gitlab-org/cli",
    );
  });

  it("enables the GitHub Issue row when preflight passes", () => {
    const { getByTestId } = renderControlled({
      repoSupported: true,
      providerCliInstalled: true,
      providerAuthenticated: true,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    const row = getByTestId("slash-item-attach:issue");
    // cmdk emits `data-disabled="false"` for enabled items; the
    // composer-side gate sets the attr to "true" when disabled. Either
    // a missing attribute or "false" both mean "enabled".
    const attr = row.getAttribute("data-disabled");
    expect(attr === null || attr === "false").toBe(true);
    // The description copy must reflect the active affordance, not
    // the disabled fallbacks.
    expect(row.textContent).toContain("Pick an issue from this repo");
  });

  // Regression for Stage 4 bug: the user reported "Not a GitHub repo"
  // copy on the row even though they were inside a verified GitHub
  // repo. Root cause was Rust-side conflation between "is a GitHub
  // repo" and "gh is authenticated". The Composer side of the contract
  // is captured here: when the preflight result is `true`, the popup
  // must NEVER show the "Not a GitHub repo" disabled copy regardless
  // of the auth signal. (The backend test suite covers the matching
  // contract on the Rust side via remote_text_points_at_github_*.)
  it("never shows 'Not a GitHub repo' when repoSupported is true", () => {
    for (const providerAuthenticated of [true, false, null] as const) {
      cleanup();
      const { getByTestId } = renderControlled({
        repoSupported: true,
        providerAuthenticated,
      });
      fireEvent.click(getByTestId("composer-attach-button"));
      const row = getByTestId("slash-item-attach:issue");
      expect(row.textContent).not.toContain("Not a GitHub repo");
    }
  });

  it("clicking GitHub Issue → fetches issues and renders rows", async () => {
    listGithubIssuesMock.mockResolvedValue([
      makeIssue({ number: 92, title: "Backend endpoints" }),
      makeIssue({ number: 70, title: "Dark mode toggle", state: "Closed" }),
    ]);
    const { getByTestId, findByText } = renderControlled({
      repoSupported: true,
      providerAuthenticated: true,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:issue"));
    await waitFor(() => {
      expect(listGithubIssuesMock).toHaveBeenCalled();
    });
    expect(await findByText("Backend endpoints")).toBeInTheDocument();
    expect(await findByText("Dark mode toggle")).toBeInTheDocument();
  });

  it("picking an issue calls onAttachIssue + inserts an @#<n> token", async () => {
    const issue = makeIssue({ number: 92, title: "Backend endpoints" });
    listGithubIssuesMock.mockResolvedValue([issue]);
    const onAttachIssue = vi.fn();
    const onDraftChange = vi.fn();
    const { getByTestId, findByText } = renderControlled({
      repoSupported: true,
      providerAuthenticated: true,
      onAttachIssue,
      onDraftChange,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:issue"));
    const row = await findByText("Backend endpoints");
    fireEvent.click(row);
    expect(onAttachIssue).toHaveBeenCalledWith(issue);
    const finalDraft =
      onDraftChange.mock.calls[onDraftChange.mock.calls.length - 1]?.[0];
    expect(finalDraft).toBe("@#92 ");
  });

  it("surfaces an error message when gh fetch fails", async () => {
    // The IssuePickerPanel maps 'rate-limited' → generic "Failed to
    // load issues" copy. Auth-flavoured errors map to the "Connect
    // GitHub to link issues" hint; missing-CLI maps to the install
    // prompt. We pin the generic branch here.
    listGithubIssuesMock.mockRejectedValue(new Error("rate-limited"));
    const { getByTestId, findByText } = renderControlled({
      repoSupported: true,
      providerAuthenticated: true,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:issue"));
    expect(await findByText("Failed to load issues")).toBeInTheDocument();
  });

  it("surfaces the auth-recovery hint when gh reports unauthenticated", async () => {
    listGithubIssuesMock.mockRejectedValue(
      new Error("gh CLI is not authenticated. Run: gh auth login"),
    );
    const { getByTestId, findByText } = renderControlled({
      repoSupported: true,
      providerAuthenticated: true,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:issue"));
    expect(
      await findByText("Connect GitHub to link issues"),
    ).toBeInTheDocument();
  });

  it("renders open issues with CircleDot + text-success and closed with CircleCheck + muted", async () => {
    listGithubIssuesMock.mockResolvedValue([
      makeIssue({ number: 92, title: "Backend endpoints", state: "Open" }),
      makeIssue({ number: 70, title: "Dark mode toggle", state: "Closed" }),
    ]);
    const { getByTestId, findByText, container } = renderControlled({
      repoSupported: true,
      providerAuthenticated: true,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:issue"));
    await findByText("Backend endpoints");
    await findByText("Dark mode toggle");

    // Lucide ships each icon with a `lucide-<name>` class. We assert
    // that exactly one open-state icon (CircleDot) and one closed-
    // state icon (CircleCheck) appear, each with the expected colour
    // class — so a future tweak that drops the success tint, or
    // confuses the open/closed icon shapes, fails this test.
    const openIcons = container.querySelectorAll(
      "svg.lucide-circle-dot.text-success",
    );
    expect(openIcons.length).toBe(1);
    const closedIcons = container.querySelectorAll(
      "svg.lucide-circle-check.text-muted-foreground",
    );
    expect(closedIcons.length).toBe(1);
  });

  it("mounts the IssuePickerPanel (with search input) on issue submode", async () => {
    listGithubIssuesMock.mockResolvedValue([]);
    const { getByTestId, getByPlaceholderText } = renderControlled({
      repoSupported: true,
      providerAuthenticated: true,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:issue"));
    // The picker panel renders a Search input — this is the canonical
    // affordance the user reported missing in the prior flat-row
    // implementation. If a future refactor regresses to flat rows
    // this test catches it.
    expect(getByPlaceholderText("Search issues...")).toBeInTheDocument();
    expect(getByTestId("composer-issue-picker")).toBeInTheDocument();
  });
});


describe("+ menu → Chat…", () => {
  const WORKSPACE = { workspaceId: "workspace-1", threadId: "current-thread" };

  beforeEach(() => {
    listSessionMentionsMock.mockReset();
    listSessionMentionsMock.mockResolvedValue([]);
  });

  it("pivots to the chat submode and lists workspace conversations", async () => {
    listSessionMentionsMock.mockResolvedValue([makeSession()]);
    const { getByTestId, findByText } = renderControlled(WORKSPACE);
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:session"));

    expect(await findByText("Harden authentication")).toBeInTheDocument();
    expect(listSessionMentionsMock).toHaveBeenCalledWith(
      "workspace-1",
      "/repo",
      "current-thread",
      30,
    );
  });

  it("picks a chat with the same token + callback contract as @session:", async () => {
    const session = makeSession();
    listSessionMentionsMock.mockResolvedValue([session]);
    const onAttachSession = vi.fn();
    const onDraftChange = vi.fn();
    const { getByTestId, findByTestId } = renderControlled({
      ...WORKSPACE,
      onAttachSession,
      onDraftChange,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:session"));
    fireEvent.click(
      await findByTestId("slash-item-attach-session:thread-aaa111"),
    );

    expect(onAttachSession).toHaveBeenCalledWith(session);
    const calls = onDraftChange.mock.calls;
    expect(calls[calls.length - 1]?.[0]).toBe(
      "@session:harden-authentication-aaa111 ",
    );
  });

  it("disables the row (and skips the query) without a workspace", () => {
    const { getByTestId } = renderControlled({ workspaceId: null });
    fireEvent.click(getByTestId("composer-attach-button"));
    const row = getByTestId("slash-item-attach:session");
    expect(row.getAttribute("data-disabled")).toBe("true");
    fireEvent.click(row);
    expect(listSessionMentionsMock).not.toHaveBeenCalled();
  });
});
