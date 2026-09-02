/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";

import type { AdoptableAgentSession } from "@/tauri/commands";

// The row only borrows the strip's horizontal inset; the real module
// drags in every picker store, which these tests have no use for.
vi.mock("./pickers/ThreadScopeRow", () => ({
  SCOPE_STRIP_INSET: "w-full px-5",
}));

import {
  ContinueTerminalSessionRow,
  landingSessionSummary,
} from "./ContinueTerminalSessionRow";

const NOW = new Date("2026-09-02T12:00:00.000Z");
const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

function ago(ms: number): string {
  return new Date(NOW.getTime() - ms).toISOString();
}

function session(
  id: string,
  overrides: Partial<AdoptableAgentSession> = {},
): AdoptableAgentSession {
  return {
    session_id: id,
    title: `Session ${id}`,
    cwd: "/home/user/projects/web-snake",
    git_branch: "master",
    last_modified: ago(30 * MINUTE),
    created_at: ago(2 * HOUR),
    file_size: 1024,
    title_source: "summary",
    existing_thread_id: null,
    same_repo: true,
    project_root: "/home/user/projects/web-snake",
    worktree_name: null,
    ...overrides,
  };
}

const FRESH = session("fresh", {
  title: "List files by size",
  last_modified: ago(32 * MINUTE),
});
const OLD_SAME_PROJECT = session("old", { last_modified: ago(6 * DAY) });
const OTHER_PROJECT_FRESH = session("other", {
  title: "Fix the ledger import",
  cwd: "/home/user/projects/codemux",
  project_root: "/home/user/projects/codemux",
  last_modified: ago(5 * MINUTE),
});
const WORKTREE = session("wt", {
  title: "Resolve PR conflicts",
  cwd: "/home/user/codemux/worktrees/resolve-pr-conflicts",
  project_root: "/home/user/projects/codemux",
  worktree_name: "resolve-pr-conflicts",
  git_branch: "resolve-pr-conflicts",
  last_modified: ago(10 * MINUTE),
});

const PROJECT = {
  kind: "project" as const,
  projectRoot: "/home/user/projects/web-snake",
};
const HOME = { kind: "home" as const };

function renderRow(
  sessions: AdoptableAgentSession[],
  scope: typeof PROJECT | typeof HOME,
) {
  const onOpenPicker = vi.fn();
  const onContinue = vi.fn();
  const utils = render(
    <ContinueTerminalSessionRow
      sessions={sessions}
      scope={scope}
      provider="claude"
      onOpenPicker={onOpenPicker}
      onContinue={onContinue}
      now={NOW}
    />,
  );
  return { ...utils, onOpenPicker, onContinue };
}

afterEach(() => cleanup());

describe("landingSessionSummary", () => {
  it("features the newest session in the selected project when it is under a day old", () => {
    const summary = landingSessionSummary(
      [OLD_SAME_PROJECT, FRESH, OTHER_PROJECT_FRESH],
      PROJECT,
      NOW,
    );
    expect(summary).toMatchObject({
      variant: "featured",
      newest: { session_id: "fresh" },
      more: 1,
      inScope: 2,
    });
  });

  it("never borrows another project's fresh session for a project draft", () => {
    // The only session in web-snake is six days old; codemux has one
    // from five minutes ago. The row must not name the codemux one.
    const summary = landingSessionSummary(
      [OLD_SAME_PROJECT, OTHER_PROJECT_FRESH],
      PROJECT,
      NOW,
    );
    expect(summary).toMatchObject({
      variant: "quiet",
      newest: { session_id: "old" },
      inScope: 1,
    });
  });

  it("counts a project's linked-worktree sessions as in the project", () => {
    const summary = landingSessionSummary(
      [WORKTREE, OTHER_PROJECT_FRESH],
      { kind: "project", projectRoot: "/home/user/projects/codemux/" },
      NOW,
    );
    expect(summary).toMatchObject({
      variant: "featured",
      newest: { session_id: "other" },
      more: 1,
    });
  });

  it("is empty when nothing in scope exists, or the project has not resolved", () => {
    expect(landingSessionSummary([OTHER_PROJECT_FRESH], PROJECT, NOW)).toEqual(
      { variant: "none" },
    );
    expect(
      landingSessionSummary([FRESH], { kind: "project", projectRoot: null }, NOW),
    ).toEqual({ variant: "none" });
    expect(landingSessionSummary([], HOME, NOW)).toEqual({ variant: "none" });
  });

  it("spans every project for a Home draft", () => {
    const summary = landingSessionSummary(
      [FRESH, OLD_SAME_PROJECT, OTHER_PROJECT_FRESH],
      HOME,
      NOW,
    );
    expect(summary).toMatchObject({
      variant: "featured",
      newest: { session_id: "other" },
      more: 2,
      inScope: 3,
    });
  });

  it("goes quiet at exactly one day", () => {
    const boundary = session("edge", { last_modified: ago(DAY) });
    expect(landingSessionSummary([boundary], PROJECT, NOW).variant).toBe(
      "quiet",
    );
    const justUnder = session("edge", { last_modified: ago(DAY - MINUTE) });
    expect(landingSessionSummary([justUnder], PROJECT, NOW).variant).toBe(
      "featured",
    );
  });
});

describe("ContinueTerminalSessionRow · featured", () => {
  it("names the session with its branch and agent, and offers Continue plus the rest", () => {
    const { getByTestId } = renderRow(
      [OLD_SAME_PROJECT, FRESH, OTHER_PROJECT_FRESH],
      PROJECT,
    );
    const row = getByTestId("draft-continue-terminal-session");
    expect(row.dataset.variant).toBe("featured");
    expect(row.textContent).toContain(
      "You have a terminal session in this project from 32 minutes ago",
    );
    expect(getByTestId("draft-continue-terminal-session-title").textContent).toBe(
      "List files by size",
    );
    expect(getByTestId("draft-continue-terminal-session-meta").textContent).toBe(
      "master · Claude Code",
    );
    expect(getByTestId("draft-continue-terminal-session-more").textContent).toContain(
      "1 more",
    );
    expect(getByTestId("draft-continue-terminal-session-continue").textContent).toBe(
      "Continue",
    );
  });

  it("Continue resumes THAT session; the rest opens the picker", () => {
    const { getByTestId, onContinue, onOpenPicker } = renderRow(
      [OLD_SAME_PROJECT, FRESH],
      PROJECT,
    );
    fireEvent.click(getByTestId("draft-continue-terminal-session-continue"));
    expect(onContinue).toHaveBeenCalledTimes(1);
    expect(onContinue.mock.calls[0]![0]).toBe(FRESH);
    expect(onOpenPicker).not.toHaveBeenCalled();

    fireEvent.click(getByTestId("draft-continue-terminal-session-more"));
    expect(onOpenPicker).toHaveBeenCalledTimes(1);
    expect(onContinue).toHaveBeenCalledTimes(1);
  });

  it("hides the rest when the featured session is the only one", () => {
    const { queryByTestId } = renderRow([FRESH], PROJECT);
    expect(queryByTestId("draft-continue-terminal-session-more")).toBeNull();
    expect(queryByTestId("draft-continue-terminal-session-continue")).not.toBeNull();
  });

  it("a Home draft names the project too, and drops 'in this project'", () => {
    const { getByTestId } = renderRow([FRESH, OTHER_PROJECT_FRESH], HOME);
    const row = getByTestId("draft-continue-terminal-session");
    expect(row.textContent).toContain(
      "You have a terminal session from 5 minutes ago",
    );
    expect(row.textContent).not.toContain("in this project");
    expect(getByTestId("draft-continue-terminal-session-meta").textContent).toBe(
      "codemux · master · Claude Code",
    );
  });

  it("wears the worktree in place of the branch", () => {
    const { getByTestId } = renderRow([WORKTREE], HOME);
    expect(getByTestId("draft-continue-terminal-session-meta").textContent).toBe(
      "codemux · ⑃ worktree resolve-pr-conflicts · Claude Code",
    );
  });

  it("uses the amber glyph only — the rest stays on neutral tokens", () => {
    const { container } = renderRow([FRESH], PROJECT);
    const html = container.innerHTML;
    expect(html).toMatch(/\btext-warning\b/);
    expect(html).not.toMatch(/\btext-primary\b/);
    expect(html).not.toMatch(/\bbg-primary\b/);
    expect(html).not.toMatch(/\btext-success\b/);
  });
});

describe("ContinueTerminalSessionRow · quiet and none", () => {
  it("folds back to a one-liner with the in-project count when nothing is recent", () => {
    const { getByTestId, onOpenPicker, onContinue } = renderRow(
      [OLD_SAME_PROJECT, session("older", { last_modified: ago(9 * DAY) }), OTHER_PROJECT_FRESH],
      PROJECT,
    );
    const row = getByTestId("draft-continue-terminal-session");
    expect(row.dataset.variant).toBe("quiet");
    expect(row.textContent).toContain("Continue a terminal session");
    expect(row.textContent).toContain("2 in this project, newest 6 days ago");
    expect(row.textContent).toContain("/resume");
    expect(row.innerHTML).not.toMatch(/\btext-warning\b/);

    fireEvent.click(row);
    expect(onOpenPicker).toHaveBeenCalledTimes(1);
    expect(onContinue).not.toHaveBeenCalled();
  });

  it("counts the whole machine for a Home draft", () => {
    const { getByTestId } = renderRow(
      [OLD_SAME_PROJECT, session("older", { last_modified: ago(9 * DAY), project_root: null, cwd: "/home/user" })],
      HOME,
    );
    expect(getByTestId("draft-continue-terminal-session").textContent).toContain(
      "2 on this machine, newest 6 days ago",
    );
  });

  it("renders nothing when no session is in scope", () => {
    const { container } = renderRow([OTHER_PROJECT_FRESH], PROJECT);
    expect(container.innerHTML).toBe("");
    cleanup();
    const empty = renderRow([], HOME);
    expect(empty.container.innerHTML).toBe("");
  });
});
