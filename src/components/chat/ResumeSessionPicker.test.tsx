/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  waitFor,
  within,
} from "@testing-library/react";
import type { ComponentProps } from "react";

import type { AdoptableAgentSession } from "@/tauri/commands";

import { ResumeSessionPicker } from "./ResumeSessionPicker";

type Props = ComponentProps<typeof ResumeSessionPicker>;

const HOME = "/home/me";
const CODEMUX = `${HOME}/projects/codemux`;
const LEDGER = `${HOME}/projects/ledger`;
const NOW = new Date("2026-04-24T12:00:00.000Z");

function makeSession(
  overrides: Partial<AdoptableAgentSession> = {},
): AdoptableAgentSession {
  return {
    session_id: "sess-1",
    title: "Refactor the splitter",
    cwd: CODEMUX,
    git_branch: "main",
    last_modified: "2026-04-24T11:00:00.000Z",
    created_at: "2026-04-24T10:00:00.000Z",
    file_size: 4096,
    title_source: "summary",
    existing_thread_id: null,
    same_repo: true,
    project_root: CODEMUX,
    worktree_name: null,
    ...overrides,
  };
}

const FIXTURE: AdoptableAgentSession[] = [
  makeSession({ session_id: "here", title: "Refactor the splitter" }),
  makeSession({
    session_id: "adopted",
    title: "Already here",
    existing_thread_id: "thread-9",
    last_modified: "2026-04-21T11:00:00.000Z",
  }),
  makeSession({
    session_id: "ledger-main",
    title: "Port the invoice exporter",
    cwd: LEDGER,
    project_root: LEDGER,
    same_repo: false,
    last_modified: "2026-04-18T11:00:00.000Z",
  }),
  makeSession({
    session_id: "ledger-wt",
    title: "Fix the rounding bug",
    cwd: `${HOME}/.codemux/worktrees/ledger/rounding`,
    git_branch: "rounding",
    worktree_name: "rounding",
    project_root: LEDGER,
    same_repo: false,
    last_modified: "2026-04-17T11:00:00.000Z",
  }),
  makeSession({
    session_id: "home",
    title: "/update-config",
    cwd: HOME,
    git_branch: null,
    project_root: null,
    same_repo: false,
    last_modified: "2026-04-10T11:00:00.000Z",
  }),
];

function baseProps(): Props {
  return {
    open: true,
    sessions: FIXTURE,
    provider: "claude",
    selectedProjectRoot: CODEMUX,
    homeDir: HOME,
    isWorkspaceOpenAt: () => false,
    query: "",
    onQueryChange: vi.fn(),
    loading: false,
    error: null,
    onSelect: vi.fn(),
    onEscape: vi.fn(),
    now: NOW,
  };
}

function renderPicker(overrides: Partial<Props> = {}) {
  const props = { ...baseProps(), ...overrides };
  const utils = render(<ResumeSessionPicker {...props} />);
  const rerender = (next: Partial<Props>) =>
    utils.rerender(<ResumeSessionPicker {...props} {...next} />);
  return { ...utils, rerender };
}

const footer = () => document.querySelector('[data-testid="slash-popup-footer"]')!;
const row = (id: string) =>
  document.querySelector(`[data-testid="slash-item-external-session:${id}"]`);
const folder = (key: string) =>
  document.querySelector(`[data-testid="resume-folder-${key}"]`)!;

afterEach(() => cleanup());

describe("ResumeSessionPicker · layout", () => {
  it("opens the selected project and collapses every other folder", () => {
    const { getByTestId } = renderPicker();
    expect(getByTestId("resume-session-count").textContent).toBe(
      "5 on this machine",
    );

    const selected = folder(CODEMUX);
    expect(selected).toHaveAttribute("data-expanded");
    expect(selected.textContent).toContain("codemux");
    expect(selected.textContent).toContain("~/projects/codemux");
    expect(selected.textContent).toContain("Selected project");
    expect(row("here")).not.toBeNull();
    expect(row("adopted")).not.toBeNull();

    const ledger = folder(LEDGER);
    expect(ledger).not.toHaveAttribute("data-expanded");
    expect(ledger.textContent).toContain("2 · 1 in worktrees · 6 days ago");
    expect(row("ledger-main")).toBeNull();

    const home = folder("home");
    expect(home.textContent).toContain("Home folder");
    expect(home.textContent).toContain("~ · ran outside any project");
    // No worktrees → no worktree part.
    expect(home.textContent).toContain("1 · ");
    expect(home.textContent).not.toContain("in worktrees");
  });

  it("expands a collapsed folder on click and folds it again", () => {
    renderPicker();
    fireEvent.click(folder(LEDGER));
    expect(row("ledger-main")).not.toBeNull();
    expect(folder(LEDGER)).toHaveAttribute("data-expanded");
    // The selected project stays open — several folders may be open.
    expect(row("here")).not.toBeNull();

    fireEvent.click(folder(LEDGER));
    expect(row("ledger-main")).toBeNull();
  });

  it("dresses rows: branch + agent, worktree pill, already-open note", () => {
    renderPicker();
    fireEvent.click(folder(LEDGER));
    expect(row("here")!.textContent).toContain("main");
    expect(row("here")!.textContent).toContain("Claude Code");
    expect(row("here")!.textContent).toContain("1 hour ago");
    expect(row("ledger-wt")!.textContent).toContain("⑃ worktree rounding");
    expect(row("adopted")!.textContent).toContain(
      "already open in Codemux — switches to it",
    );
    expect(row("adopted")!.textContent).not.toContain("Claude Code");
  });

  it("shows a cross-project RECENT block for a Home draft", () => {
    const { getByTestId } = renderPicker({ selectedProjectRoot: null });
    const recent = within(getByTestId("resume-recent"));
    const titles = recent
      .getAllByText(/.+/, { selector: '[data-testid^="slash-item-"] > span > span:first-child' })
      .map((el) => el.textContent);
    expect(titles).toEqual([
      "Refactor the splitter",
      "Already here",
      "Port the invoice exporter",
    ]);
    // Each recent row leads with its project.
    expect(
      recent.getByTestId("slash-item-external-session:ledger-main").textContent,
    ).toMatch(/^Port the invoice exporterledger·main·Claude Code/);
    // Nothing is selected, so every folder is collapsed.
    expect(folder(CODEMUX)).not.toHaveAttribute("data-expanded");
    expect(folder(CODEMUX).textContent).not.toContain("Selected project");
    expect(folder(LEDGER)).not.toHaveAttribute("data-expanded");
  });

  it("expands matching folders and hides empty ones while searching", () => {
    const { rerender } = renderPicker();
    rerender({ query: "invoice" });
    expect(row("ledger-main")).not.toBeNull();
    expect(folder(LEDGER)).toHaveAttribute("data-expanded");
    expect(document.querySelector(`[data-testid="resume-folder-${CODEMUX}"]`)).toBeNull();
    expect(document.querySelector('[data-testid="resume-folder-home"]')).toBeNull();

    rerender({ query: "no such session" });
    expect(document.body.textContent).toContain("No matches");
  });
});

describe("ResumeSessionPicker · footer", () => {
  it("names the destination of the first row by default", async () => {
    renderPicker({ isWorkspaceOpenAt: (cwd) => cwd === CODEMUX });
    await waitFor(() =>
      expect(footer().textContent).toContain(
        "Continues in codemux · main — the workspace that's already open",
      ),
    );
  });

  it("follows the keyboard highlight", async () => {
    const { getByTestId } = renderPicker();
    await waitFor(() =>
      expect(footer().textContent).toContain(
        "Opens codemux · main and continues there",
      ),
    );
    const input = getByTestId("composer-command-search");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    await waitFor(() =>
      expect(footer().textContent).toContain(
        "Switches to the open chat in codemux · main",
      ),
    );
    // Past the last open row sits the collapsed ledger folder line.
    fireEvent.keyDown(input, { key: "ArrowDown" });
    await waitFor(() => expect(footer().textContent).toContain("Expands ledger"));
    // Enter opens the folder and lands on its first session.
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(row("ledger-main")).not.toBeNull());
    await waitFor(() =>
      expect(footer().textContent).toContain("Moves this chat to ledger · main"),
    );
    fireEvent.keyDown(input, { key: "ArrowDown" });
    await waitFor(() =>
      expect(footer().textContent).toContain(
        "Moves this chat to ledger → worktree rounding (already on disk, opened as a workspace)",
      ),
    );
  });

  it("reports discovery failures and the empty state honestly", () => {
    const { rerender } = renderPicker({ sessions: [], error: "sidecar exited" });
    expect(footer()).toHaveAttribute("data-tone", "error");
    expect(footer().textContent).toBe("Resume: sidecar exited");

    rerender({ error: null, loading: true });
    expect(footer().textContent).toBe("Reading local history…");

    rerender({ error: null, loading: false });
    expect(footer().textContent).toBe(
      "No terminal sessions found on this machine.",
    );
  });
});

describe("ResumeSessionPicker · picking", () => {
  it("hands the session to onSelect on click", () => {
    const onSelect = vi.fn();
    renderPicker({ onSelect });
    fireEvent.click(row("here")!);
    expect(onSelect).toHaveBeenCalledWith(FIXTURE[0]);
  });

  it("selects the highlighted row on Enter and closes on Escape", async () => {
    const onSelect = vi.fn();
    const onEscape = vi.fn();
    const { getByTestId } = renderPicker({ onSelect, onEscape });
    const input = getByTestId("composer-command-search");
    await waitFor(() =>
      expect(row("here")).toHaveAttribute("data-selected", "true"),
    );
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSelect).toHaveBeenCalledWith(FIXTURE[0]);

    fireEvent.keyDown(input, { key: "Escape" });
    expect(onEscape).toHaveBeenCalledTimes(1);
  });
});
