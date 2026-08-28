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
import type { FileMatch } from "@/tauri/types";

vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return {
    ...actual,
    listProjectFiles: vi.fn(),
    listGithubIssuesByPath: vi.fn().mockResolvedValue([]),
    agentChatListSessionMentions: vi.fn().mockResolvedValue([]),
    getGithubIssueByPath: vi.fn(),
    // Skills loader is unrelated but the Composer pulls it from the
    // store on slash-popup open; stub it so we don't trip the real
    // invoke from inside the mention tests.
    listSkills: vi.fn().mockResolvedValue([]),
  };
});

import { Composer } from "./Composer";
import {
  agentChatListSessionMentions,
  getGithubIssueByPath,
  listGithubIssuesByPath,
  listProjectFiles,
} from "@/tauri/commands";
import type { AgentChatSessionMention } from "@/tauri/commands";
import type { GitHubIssue } from "@/tauri/types";

type ComposerProps = ComponentProps<typeof Composer>;

const listProjectFilesMock =
  listProjectFiles as unknown as ReturnType<typeof vi.fn>;
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
    updatedAt: null,
    ...overrides,
  };
}

function makeMatch(overrides: Partial<FileMatch> = {}): FileMatch {
  return {
    path: "src/components/chat/Composer.tsx",
    absolute_path: "/repo/src/components/chat/Composer.tsx",
    score: 200,
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

function renderComposer(
  props: Partial<ComposerProps> = {},
): RenderResult {
  return render(
    <TooltipProvider>
      <Composer {...baseProps()} {...props} />
    </TooltipProvider>,
  );
}

/** Controlled wrapper that mirrors the real `AgentChatPane` behavior:
 *  the textarea writes to a parent state that flows back as the
 *  `draft` prop. Without this, the Composer's strip-on-pick reads a
 *  stale prop and asserts trip on empty strings. */
function ControlledComposer(props: Partial<ComposerProps> = {}) {
  const [draft, setDraft] = useState(props.draft ?? "");
  const baselineProps = baseProps();
  return (
    <Composer
      {...baselineProps}
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
  listProjectFilesMock.mockResolvedValue([]);
  listGithubIssuesMock.mockReset();
  listGithubIssuesMock.mockResolvedValue([]);
  getGithubIssueMock.mockReset();
  listSessionMentionsMock.mockReset();
  listSessionMentionsMock.mockResolvedValue([]);
});

afterEach(() => cleanup());

function typeIntoTextarea(value: string, cursor?: number): HTMLTextAreaElement {
  const textarea = document.querySelector(
    'textarea',
  ) as HTMLTextAreaElement;
  fireEvent.change(textarea, {
    target: {
      value,
      selectionStart: cursor ?? value.length,
      selectionEnd: cursor ?? value.length,
    },
  });
  return textarea;
}

describe("Composer @ mention popup (Step 8 Stage 2)", () => {
  it("fetches files via listProjectFiles when the user types @<query>", async () => {
    listProjectFilesMock.mockResolvedValue([makeMatch()]);
    const onDraftChange = vi.fn();
    renderComposer({ draft: "", onDraftChange });
    typeIntoTextarea("@composer");
    await waitFor(() => {
      expect(listProjectFilesMock).toHaveBeenCalled();
    });
    const [cwdArg, queryArg, limitArg] = listProjectFilesMock.mock.calls[0]!;
    expect(cwdArg).toBe("/repo");
    expect(queryArg).toBe("composer");
    expect(typeof limitArg).toBe("number");
  });

  it("requests an alphabetical listing (null query) on bare @", async () => {
    listProjectFilesMock.mockResolvedValue([]);
    renderComposer();
    typeIntoTextarea("@");
    await waitFor(() => {
      expect(listProjectFilesMock).toHaveBeenCalled();
    });
    const [, queryArg] = listProjectFilesMock.mock.calls[0]!;
    expect(queryArg).toBeNull();
  });

  it("renders fetched matches as picker rows", async () => {
    listProjectFilesMock.mockResolvedValue([
      makeMatch({ path: "src/components/chat/Composer.tsx" }),
      makeMatch({
        path: "src/components/chat/ComposerFooter.tsx",
        absolute_path: "/repo/src/components/chat/ComposerFooter.tsx",
      }),
    ]);
    const { findByText } = renderComposer();
    typeIntoTextarea("@comp");
    expect(
      await findByText("src/components/chat/Composer.tsx"),
    ).toBeInTheDocument();
    expect(
      await findByText("src/components/chat/ComposerFooter.tsx"),
    ).toBeInTheDocument();
  });

  it("calls onAttachFile and inserts an @<basename> token in the textarea", async () => {
    // Stage 2.1 — inline-chip model: picking a file replaces the
    // typed `@<query>` with `@<basename> ` so the token stays in the
    // textarea and the mirror renders it as an inline chip.
    const match = makeMatch();
    listProjectFilesMock.mockResolvedValue([match]);
    const onAttachFile = vi.fn();
    const onDraftChange = vi.fn();
    const { findByText } = renderControlled({
      onAttachFile,
      onDraftChange,
    });
    typeIntoTextarea("@composer");
    const row = await findByText("src/components/chat/Composer.tsx");
    fireEvent.click(row);
    expect(onAttachFile).toHaveBeenCalledWith(match);
    const calls = onDraftChange.mock.calls;
    const lastDraftCall = calls[calls.length - 1];
    expect(lastDraftCall?.[0]).toBe("@Composer.tsx ");
  });

  it("replaces the typed @<query> with the @<basename> token, preserving prose", async () => {
    // Stage 2.1 — typing prose then `@<query>` and picking inserts
    // the basename back where the query was: "hello @utils" → pick
    // utils.ts → "hello @utils.ts " (trailing space lets the user
    // keep typing without a manual keystroke).
    const match = makeMatch({
      path: "src/lib/utils.ts",
      absolute_path: "/repo/src/lib/utils.ts",
    });
    listProjectFilesMock.mockResolvedValue([match]);
    const onAttachFile = vi.fn();
    const onDraftChange = vi.fn();
    const { findByText } = renderControlled({
      onAttachFile,
      onDraftChange,
    });
    typeIntoTextarea("hello @utils");
    const row = await findByText("src/lib/utils.ts");
    fireEvent.click(row);
    expect(onAttachFile).toHaveBeenCalledWith(match);
    const calls = onDraftChange.mock.calls;
    const finalText = calls[calls.length - 1]?.[0];
    expect(finalText).toBe("hello @utils.ts ");
  });

  it("Esc closes the mention popup without picking", async () => {
    listProjectFilesMock.mockResolvedValue([makeMatch()]);
    const onAttachFile = vi.fn();
    const { findByText, queryByText } = renderComposer({ onAttachFile });
    typeIntoTextarea("@comp");
    await findByText("src/components/chat/Composer.tsx");
    const textarea = document.querySelector(
      'textarea',
    ) as HTMLTextAreaElement;
    fireEvent.keyDown(textarea, { key: "Escape" });
    await waitFor(() => {
      expect(
        queryByText("src/components/chat/Composer.tsx"),
      ).toBeNull();
    });
    expect(onAttachFile).not.toHaveBeenCalled();
  });

  it("shows the project-required footer hint when cwd is null", async () => {
    const { findByText } = renderComposer({ cwd: null });
    typeIntoTextarea("@");
    expect(
      await findByText("Open this chat in a project to attach files."),
    ).toBeInTheDocument();
    // No fetch should fire when there's nothing to scan.
    expect(listProjectFilesMock).not.toHaveBeenCalled();
  });

  it("does not fire the popup on a slash trigger (slash and mention are exclusive)", async () => {
    renderComposer();
    typeIntoTextarea("/plan");
    // The slash popup uses a different fetch path; mention's listProjectFiles
    // mock should not have been touched.
    expect(listProjectFilesMock).not.toHaveBeenCalled();
  });
});

describe("Composer @issue: mention popup (Step 8 Stage 4)", () => {
  it("routes @issue:<query> through listGithubIssuesByPath", async () => {
    listGithubIssuesMock.mockResolvedValue([makeIssue({ title: "bug fix" })]);
    renderComposer({ repoSupported: true, providerAuthenticated: true });
    typeIntoTextarea("@issue:bug");
    await waitFor(() => {
      expect(listGithubIssuesMock).toHaveBeenCalled();
    });
    const [pathArg, searchArg] = listGithubIssuesMock.mock.calls[0]!;
    expect(pathArg).toBe("/repo");
    expect(searchArg).toBe("bug");
    // File search must NOT fire for issue-prefixed mentions.
    expect(listProjectFilesMock).not.toHaveBeenCalled();
  });

  it("direct-fetches when the filter is numeric (@issue:1234)", async () => {
    getGithubIssueMock.mockResolvedValue(makeIssue());
    renderComposer({ repoSupported: true, providerAuthenticated: true });
    typeIntoTextarea("@issue:1234");
    await waitFor(() => {
      expect(getGithubIssueMock).toHaveBeenCalled();
    });
    const [pathArg, numArg] = getGithubIssueMock.mock.calls[0]!;
    expect(pathArg).toBe("/repo");
    expect(numArg).toBe(1234);
    // The list path is bypassed for numeric direct fetches.
    expect(listGithubIssuesMock).not.toHaveBeenCalled();
  });

  it("renders fetched issues as picker rows", async () => {
    listGithubIssuesMock.mockResolvedValue([
      makeIssue({ number: 92, title: "Backend endpoints" }),
      makeIssue({ number: 70, title: "Dark mode toggle", state: "Closed" }),
    ]);
    const { findByText } = renderComposer({
      repoSupported: true,
      providerAuthenticated: true,
    });
    typeIntoTextarea("@issue:");
    expect(await findByText("Backend endpoints")).toBeInTheDocument();
    expect(await findByText("Dark mode toggle")).toBeInTheDocument();
  });

  it("calls onAttachIssue and inserts an @#<n> token on pick", async () => {
    const issue = makeIssue({ number: 92, title: "Backend endpoints" });
    listGithubIssuesMock.mockResolvedValue([issue]);
    const onAttachIssue = vi.fn();
    const onDraftChange = vi.fn();
    const { findByText } = renderControlled({
      repoSupported: true,
      providerAuthenticated: true,
      onAttachIssue,
      onDraftChange,
    });
    typeIntoTextarea("@issue:back");
    const row = await findByText("Backend endpoints");
    fireEvent.click(row);
    expect(onAttachIssue).toHaveBeenCalledWith(issue);
    const finalDraft =
      onDraftChange.mock.calls[onDraftChange.mock.calls.length - 1]?.[0];
    expect(finalDraft).toBe("@#92 ");
  });

  it("shows the not-a-github-repo footer when preflight reports false", async () => {
    const { findByText } = renderComposer({ repoSupported: false });
    typeIntoTextarea("@issue:foo");
    expect(await findByText("Not a GitHub repo.")).toBeInTheDocument();
    // Don't call gh when we already know it'd fail.
    expect(listGithubIssuesMock).not.toHaveBeenCalled();
  });

  it("shows the gh-auth footer when authentication is missing", async () => {
    const { findByText } = renderComposer({
      repoSupported: true,
      providerAuthenticated: false,
    });
    typeIntoTextarea("@issue:foo");
    expect(await findByText("Sign in with: gh auth login")).toBeInTheDocument();
    expect(listGithubIssuesMock).not.toHaveBeenCalled();
  });

  it("points at the download, not a login, when the CLI is missing", async () => {
    const { findByText } = renderComposer({
      repoSupported: true,
      providerCliInstalled: false,
      providerAuthenticated: false,
    });
    typeIntoTextarea("@issue:foo");
    expect(
      await findByText("Install gh from cli.github.com"),
    ).toBeInTheDocument();
    expect(listGithubIssuesMock).not.toHaveBeenCalled();
  });

  it("renders open issues with CircleDot + text-success and closed with CircleCheck + muted", async () => {
    // Stage 4 polish — the mention popup must visually match the
    // IssuePickerPanel that the `+ → Issue…` path mounts. Both
    // surfaces use CircleDot (filled) tinted `text-success` for open
    // and CircleCheck (with tick) muted for closed, so users get a
    // consistent open/closed signal regardless of how they reached
    // the picker.
    listGithubIssuesMock.mockResolvedValue([
      makeIssue({ number: 92, title: "Open thing", state: "Open" }),
      makeIssue({ number: 70, title: "Closed thing", state: "Closed" }),
    ]);
    const { container, findByText } = renderComposer({
      repoSupported: true,
      providerAuthenticated: true,
    });
    typeIntoTextarea("@issue:");
    await findByText("Open thing");
    await findByText("Closed thing");
    expect(
      container.querySelectorAll("svg.lucide-circle-dot.text-success").length,
    ).toBe(1);
    expect(
      container.querySelectorAll(
        "svg.lucide-circle-check.text-muted-foreground",
      ).length,
    ).toBe(1);
  });
});


describe("bare @ chats group", () => {
  const WORKSPACE = { workspaceId: "workspace-1", threadId: "current-thread" };

  it("shows chats above files on a bare @ without the session: prefix", async () => {
    listProjectFilesMock.mockResolvedValue([makeMatch()]);
    listSessionMentionsMock.mockResolvedValue([makeSession()]);
    const { findByTestId, container } = renderComposer(WORKSPACE);
    typeIntoTextarea("@");

    await findByTestId("slash-item-session:thread-aaa111");
    await findByTestId("slash-item-file:/repo/src/components/chat/Composer.tsx");
    const headings = Array.from(
      container.querySelectorAll("[cmdk-group-heading]"),
    ).map((el) => el.textContent);
    expect(headings).toEqual(["CHATS", "FILES"]);
  });

  it("caps the bare-@ chats group so file search stays dominant", async () => {
    listProjectFilesMock.mockResolvedValue([makeMatch()]);
    listSessionMentionsMock.mockResolvedValue([
      makeSession({ thread_id: "t-1", title: "One" }),
      makeSession({ thread_id: "t-2", title: "Two" }),
      makeSession({ thread_id: "t-3", title: "Three" }),
      makeSession({ thread_id: "t-4", title: "Four" }),
    ]);
    const { findByTestId, queryByTestId } = renderComposer(WORKSPACE);
    typeIntoTextarea("@");

    await findByTestId("slash-item-session:t-3");
    expect(queryByTestId("slash-item-session:t-4")).toBeNull();
  });

  it("shows every match under @session:, ignoring the bare-@ cap", async () => {
    listSessionMentionsMock.mockResolvedValue([
      makeSession({ thread_id: "t-1", title: "One" }),
      makeSession({ thread_id: "t-2", title: "Two" }),
      makeSession({ thread_id: "t-3", title: "Three" }),
      makeSession({ thread_id: "t-4", title: "Four" }),
    ]);
    const { findByTestId } = renderComposer(WORKSPACE);
    typeIntoTextarea("@session:");
    expect(await findByTestId("slash-item-session:t-4")).toBeInTheDocument();
  });

  it("filters the chats group by the same text that filters files", async () => {
    listProjectFilesMock.mockResolvedValue([]);
    listSessionMentionsMock.mockResolvedValue([
      makeSession({ thread_id: "t-1", title: "Harden authentication" }),
      makeSession({ thread_id: "t-2", title: "Search indexing" }),
    ]);
    const { findByTestId, queryByTestId } = renderComposer(WORKSPACE);
    typeIntoTextarea("@harden");
    await findByTestId("slash-item-session:t-1");
    expect(queryByTestId("slash-item-session:t-2")).toBeNull();
  });

  it("keeps the default highlight on the top file when chats resolve first", async () => {
    // Chats come from a local query and land well before the file
    // scan. `@foo` + Enter has always meant "top file match" — an
    // auto-highlighted chat row would silently attach a whole
    // conversation instead.
    let resolveFiles: (matches: FileMatch[]) => void = () => {};
    listProjectFilesMock.mockReturnValue(
      new Promise<FileMatch[]>((resolve) => {
        resolveFiles = resolve;
      }),
    );
    listSessionMentionsMock.mockResolvedValue([makeSession()]);
    const { findByTestId } = renderComposer(WORKSPACE);
    // Matches both the chat title and (via the stubbed backend) a file.
    typeIntoTextarea("@harden");

    const chatRow = await findByTestId("slash-item-session:thread-aaa111");
    expect(chatRow).toHaveAttribute("data-selected", "true");

    resolveFiles([makeMatch()]);
    const fileRow = await findByTestId(
      "slash-item-file:/repo/src/components/chat/Composer.tsx",
    );
    await waitFor(() => {
      expect(fileRow).toHaveAttribute("data-selected", "true");
    });
    expect(chatRow).toHaveAttribute("data-selected", "false");
  });

  it("respects an arrow-key highlight when files land afterwards", async () => {
    // The auto-pick above must not fight the user: once they have
    // moved the highlight themselves, a late file result leaves it
    // where they put it.
    let resolveFiles: (matches: FileMatch[]) => void = () => {};
    listProjectFilesMock.mockReturnValue(
      new Promise<FileMatch[]>((resolve) => {
        resolveFiles = resolve;
      }),
    );
    listSessionMentionsMock.mockResolvedValue([makeSession()]);
    const { findByTestId } = renderComposer(WORKSPACE);
    const textarea = typeIntoTextarea("@harden");

    const chatRow = await findByTestId("slash-item-session:thread-aaa111");
    fireEvent.keyDown(textarea, { key: "ArrowDown" });
    expect(chatRow).toHaveAttribute("data-selected", "true");

    resolveFiles([makeMatch()]);
    await findByTestId("slash-item-file:/repo/src/components/chat/Composer.tsx");
    expect(chatRow).toHaveAttribute("data-selected", "true");
  });

  it("inserts the session token when a bare-@ chat row is picked", async () => {
    listProjectFilesMock.mockResolvedValue([]);
    const session = makeSession();
    listSessionMentionsMock.mockResolvedValue([session]);
    const onAttachSession = vi.fn();
    const onDraftChange = vi.fn();
    const { findByTestId } = renderControlled({
      ...WORKSPACE,
      onAttachSession,
      onDraftChange,
    });
    typeIntoTextarea("@");
    fireEvent.click(await findByTestId("slash-item-session:thread-aaa111"));

    expect(onAttachSession).toHaveBeenCalledWith(session);
    const calls = onDraftChange.mock.calls;
    expect(calls[calls.length - 1]?.[0]).toBe(
      "@session:harden-authentication-aaa111 ",
    );
  });
});
