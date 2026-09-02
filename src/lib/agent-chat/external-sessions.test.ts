import { describe, expect, it } from "vitest";

import {
  abbreviateHome,
  describeResumeDestination,
  externalSessionFolderLabel,
  externalSessionRowId,
  groupAdoptableSessions,
  RESUME_HOME_FOLDER_KEY,
  RESUME_HOME_FOLDER_NAME,
  resumeDestinationText,
  toExternalAgentSession,
  type ResumeRow,
} from "./external-sessions";
import type { AdoptableAgentSession } from "@/tauri/commands";

const HOME = "/home/me";
const NOW = new Date("2026-04-24T12:00:00.000Z");

function makeSession(
  overrides: Partial<AdoptableAgentSession> = {},
): AdoptableAgentSession {
  return {
    session_id: "sess-1",
    title: "Refactor the splitter",
    cwd: `${HOME}/projects/codemux`,
    git_branch: "main",
    last_modified: "2026-04-24T11:00:00.000Z",
    created_at: "2026-04-24T10:00:00.000Z",
    file_size: 4096,
    title_source: "summary",
    existing_thread_id: null,
    same_repo: true,
    project_root: `${HOME}/projects/codemux`,
    worktree_name: null,
    ...overrides,
  };
}

/** A worktree session of the codemux project. */
function worktreeSession(
  name: string,
  overrides: Partial<AdoptableAgentSession> = {},
): AdoptableAgentSession {
  return makeSession({
    session_id: `wt-${name}`,
    cwd: `${HOME}/.codemux/worktrees/codemux/${name}`,
    git_branch: name,
    worktree_name: name,
    ...overrides,
  });
}

function homeSession(
  overrides: Partial<AdoptableAgentSession> = {},
): AdoptableAgentSession {
  return makeSession({
    session_id: "home-1",
    title: "/update-config",
    cwd: HOME,
    git_branch: null,
    project_root: null,
    same_repo: false,
    ...overrides,
  });
}

function ledgerSession(
  overrides: Partial<AdoptableAgentSession> = {},
): AdoptableAgentSession {
  return makeSession({
    session_id: "ledger-1",
    title: "Port the invoice exporter",
    cwd: `${HOME}/projects/ledger`,
    project_root: `${HOME}/projects/ledger`,
    same_repo: false,
    ...overrides,
  });
}

function group(
  sessions: AdoptableAgentSession[],
  args: Partial<Parameters<typeof groupAdoptableSessions>[0]> = {},
) {
  return groupAdoptableSessions({
    sessions,
    selectedProjectRoot: `${HOME}/projects/codemux`,
    homeDir: HOME,
    now: NOW,
    ...args,
  });
}

describe("groupAdoptableSessions · folders", () => {
  it("folds a project's worktrees into one folder and counts them", () => {
    const { folders, total } = group([
      makeSession(),
      worktreeSession("search-index"),
      worktreeSession("perf-sweep"),
    ]);
    expect(total).toBe(3);
    expect(folders).toHaveLength(1);
    const [codemux] = folders;
    expect(codemux!.key).toBe(`${HOME}/projects/codemux`);
    expect(codemux!.name).toBe("codemux");
    expect(codemux!.path).toBe("~/projects/codemux");
    expect(codemux!.count).toBe(3);
    expect(codemux!.worktreeCount).toBe(2);
    expect(codemux!.isSelected).toBe(true);
  });

  it("buckets sessions outside any repository under the home folder", () => {
    const { folders } = group([homeSession()]);
    expect(folders).toHaveLength(1);
    expect(folders[0]).toMatchObject({
      key: RESUME_HOME_FOLDER_KEY,
      name: RESUME_HOME_FOLDER_NAME,
      path: "~",
      isHome: true,
      isSelected: false,
    });
    expect(folders[0]!.rows[0]!.project).toBe(RESUME_HOME_FOLDER_NAME);
  });

  it("orders the selected project first, then by newest, home last", () => {
    const { folders } = group([
      homeSession({ last_modified: "2026-04-24T11:59:00.000Z" }),
      ledgerSession({ last_modified: "2026-04-23T00:00:00.000Z" }),
      makeSession({
        session_id: "hermes",
        cwd: `${HOME}/projects/hermes-agent`,
        project_root: `${HOME}/projects/hermes-agent`,
        same_repo: false,
        last_modified: "2026-04-24T11:30:00.000Z",
      }),
      makeSession({ last_modified: "2026-04-01T00:00:00.000Z" }),
    ]);
    expect(folders.map((folder) => folder.name)).toEqual([
      "codemux",
      "hermes-agent",
      "ledger",
      RESUME_HOME_FOLDER_NAME,
    ]);
  });

  it("tolerates a trailing slash on the selected root", () => {
    const { folders } = group([makeSession()], {
      selectedProjectRoot: `${HOME}/projects/codemux/`,
    });
    expect(folders[0]!.isSelected).toBe(true);
  });

  it("sorts rows within a folder newest first and labels the folder's newest", () => {
    const { folders } = group([
      makeSession({ session_id: "old", last_modified: "2026-04-20T09:00:00.000Z" }),
      makeSession({ session_id: "new", last_modified: "2026-04-24T11:00:00.000Z" }),
    ]);
    expect(folders[0]!.rows.map((row) => row.id)).toEqual([
      externalSessionRowId("new"),
      externalSessionRowId("old"),
    ]);
    expect(folders[0]!.newest).toBe("1 hour ago");
    expect(folders[0]!.rows[1]!.when).toBe("4 days ago");
  });

  it("marks rows Codemux already owns", () => {
    const { folders } = group([makeSession({ existing_thread_id: "t-1" })]);
    expect(folders[0]!.rows[0]!.alreadyOpen).toBe(true);
  });
});

describe("groupAdoptableSessions · Home draft", () => {
  it("shows the newest three across every folder as RECENT", () => {
    const { recent, folders } = group(
      [
        makeSession({ session_id: "a", last_modified: "2026-04-24T11:00:00.000Z" }),
        ledgerSession({ session_id: "b", last_modified: "2026-04-24T10:00:00.000Z" }),
        homeSession({ session_id: "c", last_modified: "2026-04-24T09:00:00.000Z" }),
        worktreeSession("d", { last_modified: "2026-04-24T08:00:00.000Z" }),
      ],
      { selectedProjectRoot: null },
    );
    expect(recent!.map((row) => row.session.session_id)).toEqual(["a", "b", "c"]);
    expect(recent!.map((row) => row.project)).toEqual([
      "codemux",
      "ledger",
      RESUME_HOME_FOLDER_NAME,
    ]);
    // No folder is selected, so nothing jumps the recency order.
    expect(folders.every((folder) => !folder.isSelected)).toBe(true);
    expect(folders.map((folder) => folder.name)).toEqual([
      "codemux",
      "ledger",
      RESUME_HOME_FOLDER_NAME,
    ]);
  });

  it("has no RECENT block once a project is selected", () => {
    expect(group([makeSession()]).recent).toBeNull();
  });
});

describe("groupAdoptableSessions · search", () => {
  const sessions = [
    makeSession({ session_id: "a", title: "Refactor the splitter" }),
    worktreeSession("search-index", { title: "Wire the usage ledger key" }),
    ledgerSession({ session_id: "l", title: "Port the invoice exporter" }),
    homeSession(),
  ];

  it("matches titles across every folder and drops empty folders", () => {
    const { folders, total } = group(sessions, { query: "invoice" });
    expect(total).toBe(4);
    expect(folders.map((folder) => folder.name)).toEqual(["ledger"]);
    expect(folders[0]!.count).toBe(1);
  });

  it("matches the project name, the branch and the worktree name", () => {
    expect(group(sessions, { query: "LEDGER" }).folders.map((f) => f.name)).toEqual([
      "codemux",
      "ledger",
    ]);
    expect(
      group(sessions, { query: "search-index" }).folders[0]!.rows.map(
        (row) => row.session.session_id,
      ),
    ).toEqual(["wt-search-index"]);
    expect(group(sessions, { query: "main" }).folders.map((f) => f.name)).toEqual([
      "codemux",
      "ledger",
    ]);
  });

  it("suppresses the RECENT block while searching", () => {
    expect(
      group(sessions, { selectedProjectRoot: null, query: "port" }).recent,
    ).toBeNull();
  });
});

describe("describeResumeDestination", () => {
  const selected = `${HOME}/projects/codemux`;
  function rowFor(
    session: AdoptableAgentSession,
    selectedProjectRoot: string | null = selected,
  ): ResumeRow {
    const { folders, recent } = group([session], { selectedProjectRoot });
    return (recent ?? folders[0]!.rows)[0]!;
  }
  const text = (
    session: AdoptableAgentSession,
    ctx: { selectedProjectRoot?: string | null; workspaceOpen?: boolean } = {},
  ) => {
    const selectedProjectRoot =
      ctx.selectedProjectRoot === undefined ? selected : ctx.selectedProjectRoot;
    return resumeDestinationText(
      describeResumeDestination(rowFor(session, selectedProjectRoot), {
        selectedProjectRoot,
        workspaceOpen: ctx.workspaceOpen ?? false,
      }),
    );
  };

  it("switches to a thread Codemux already owns, wherever it lives", () => {
    expect(text(ledgerSession({ existing_thread_id: "t-1" }))).toBe(
      "Switches to the open chat in ledger · main",
    );
    expect(
      describeResumeDestination(rowFor(makeSession({ existing_thread_id: "t" })), {
        selectedProjectRoot: selected,
        workspaceOpen: true,
      }).kind,
    ).toBe("switch");
  });

  it("continues in the open workspace of the selected project", () => {
    expect(text(makeSession(), { workspaceOpen: true })).toBe(
      "Continues in codemux · main — the workspace that's already open",
    );
  });

  it("opens the selected project when nothing is open there", () => {
    expect(text(makeSession())).toBe("Opens codemux · main and continues there");
  });

  it("opens whatever a Home draft picks, naming the home folder plainly", () => {
    expect(text(ledgerSession(), { selectedProjectRoot: null })).toBe(
      "Opens ledger · main and continues there",
    );
    expect(text(homeSession(), { selectedProjectRoot: null })).toBe(
      "Opens your home folder and continues there",
    );
  });

  it("moves the chat to another project's main checkout", () => {
    expect(text(ledgerSession())).toBe("Moves this chat to ledger · main");
    expect(text(homeSession())).toBe("Moves this chat to your home folder");
  });

  it("moves the chat into another project's existing worktree", () => {
    const session = worktreeSession("resolve-pr-conflicts", {
      cwd: `${HOME}/.codemux/worktrees/ledger/resolve-pr-conflicts`,
      project_root: `${HOME}/projects/ledger`,
      same_repo: false,
    });
    expect(text(session)).toBe(
      "Moves this chat to ledger → worktree resolve-pr-conflicts (already on disk, opened as a workspace)",
    );
  });

  it("stays inside the selected project for its own worktrees", () => {
    expect(text(worktreeSession("perf-sweep"), { workspaceOpen: true })).toBe(
      "Continues in codemux · perf-sweep — the workspace that's already open",
    );
  });

  it("leaves the branch out when the session has none", () => {
    expect(text(makeSession({ git_branch: null }))).toBe(
      "Opens codemux and continues there",
    );
  });
});

describe("toExternalAgentSession", () => {
  it("drops the Codemux-side decorations before the adopt call", () => {
    const payload = toExternalAgentSession(
      makeSession({ existing_thread_id: "t-1", same_repo: false }),
    );
    expect(payload).not.toHaveProperty("existing_thread_id");
    expect(payload).not.toHaveProperty("same_repo");
    expect(payload).not.toHaveProperty("project_root");
    expect(payload).not.toHaveProperty("worktree_name");
    expect(payload.session_id).toBe("sess-1");
    expect(payload.cwd).toBe(`${HOME}/projects/codemux`);
  });
});

describe("externalSessionFolderLabel", () => {
  it("takes the last segment regardless of separator or trailing slash", () => {
    expect(externalSessionFolderLabel("/projects/codemux/")).toBe("codemux");
    expect(externalSessionFolderLabel("C:\\work\\ledger")).toBe("ledger");
  });
});

describe("abbreviateHome", () => {
  it("folds the home prefix into ~ and leaves other paths alone", () => {
    expect(abbreviateHome("/home/me/projects/app", "/home/me")).toBe(
      "~/projects/app",
    );
    expect(abbreviateHome("/home/me", "/home/me/")).toBe("~");
    expect(abbreviateHome("/srv/app", "/home/me")).toBe("/srv/app");
    expect(abbreviateHome("/home/me/x", null)).toBe("/home/me/x");
    // A sibling that merely shares the prefix string is not inside home.
    expect(abbreviateHome("/home/meow/x", "/home/me")).toBe("/home/meow/x");
  });
});
