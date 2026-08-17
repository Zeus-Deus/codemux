import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

import type { CommitSummary, PullRequestInfo } from "@/tauri/types";

/** Every backend call the form makes, in the order it made them. */
const calls = vi.hoisted(() => [] as string[]);
const commits = vi.hoisted(() => ({ current: [] as CommitSummary[] }));
const templates = vi.hoisted(() => ({ current: {} as Record<string, string> }));
const dirty = vi.hoisted(() => ({ current: [] as unknown[] }));
const upstream = vi.hoisted(() => ({ current: true }));
const createResult = vi.hoisted(() => ({
  current: null as null | PullRequestInfo | Error,
}));

const gitPushChanges = vi.hoisted(() =>
  vi.fn((_path: string, setUpstream: boolean) => {
    calls.push(`push:${setUpstream}`);
    return Promise.resolve();
  }),
);
const createPullRequest = vi.hoisted(() =>
  vi.fn((_p: string, title: string, body: string, base: string, draft: boolean) => {
    calls.push(`create:${base}:${draft}`);
    const result = createResult.current;
    if (result instanceof Error) return Promise.reject(result.message);
    return Promise.resolve({ ...(result as PullRequestInfo), title, body });
  }),
);
const requestPrReview = vi.hoisted(() =>
  vi.fn((_p: string, _n: number, reviewer: string) => {
    calls.push(`reviewer:${reviewer}`);
    return Promise.resolve();
  }),
);

vi.mock("@/tauri/commands", () => ({
  gitPushChanges,
  createPullRequest,
  requestPrReview,
  gitCommitsAhead: vi.fn(() => Promise.resolve(commits.current)),
  getGitStatus: vi.fn(() => Promise.resolve(dirty.current)),
  getGitBranchInfo: vi.fn(() =>
    Promise.resolve({ branch: "feat/chat", ahead: 2, behind: 0, has_upstream: upstream.current }),
  ),
  listBranches: vi.fn(() => Promise.resolve(["main", "develop"])),
  listDirectory: vi.fn(() => Promise.resolve([])),
  readFile: vi.fn((path: string) => {
    const found = templates.current[path];
    return found ? Promise.resolve(found) : Promise.reject("ENOENT");
  }),
}));

vi.mock("@/lib/toast", () => ({
  toast: { info: vi.fn(), error: vi.fn(), success: vi.fn(), warning: vi.fn() },
}));

import { CreatePrForm } from "./create-pr-form";
import { resolveProvider } from "@/lib/source-control";

const PR: PullRequestInfo = {
  number: 900,
  title: "feat(chat): stream replies",
  url: "https://github.com/example/codemux/pull/900",
  state: "OPEN",
  head_branch: "feat/chat",
  base_branch: "main",
  is_draft: false,
} as PullRequestInfo;

const commit = (subject: string, body = ""): CommitSummary => ({
  short_hash: subject.slice(0, 7),
  subject,
  body,
});

function renderForm(over: Partial<Parameters<typeof CreatePrForm>[0]> = {}) {
  const onCreated = vi.fn();
  const onOpenChanges = vi.fn();
  render(
    <CreatePrForm
      cwd="/home/dev/wt/chat"
      projectRoot="/home/dev/projects/codemux"
      branchName="feat/chat-channel"
      defaultBranch="main"
      provider={resolveProvider("github")}
      onCreated={onCreated}
      onCancel={vi.fn()}
      onOpenChanges={onOpenChanges}
      {...over}
    />,
  );
  return { onCreated, onOpenChanges };
}

const titleField = () => screen.getByLabelText("Title") as HTMLInputElement;
const bodyField = () => screen.getByLabelText("Description") as HTMLTextAreaElement;

describe("CreatePrForm", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    calls.length = 0;
    commits.current = [
      commit("feat(chat): stream replies without buffering", "Verification · npm run check"),
      commit("feat(chat): stream replies over one channel"),
    ];
    templates.current = {};
    dirty.current = [];
    upstream.current = true;
    createResult.current = PR;
  });

  it("arrives drafted from the commits, and says so", async () => {
    renderForm();

    await waitFor(() =>
      expect(titleField().value).toBe("feat(chat): stream replies"),
    );
    expect(screen.getByTestId("create-pr-drafted-note")).toBeInTheDocument();
    expect(bodyField().value).toContain("- feat(chat): stream replies over one channel");
    expect(bodyField().value).toContain("Verification · npm run check");
  });

  it("uses a lone commit's subject as the title", async () => {
    commits.current = [commit("docs: drop the ports section")];
    renderForm();
    await waitFor(() => expect(titleField().value).toBe("docs: drop the ports section"));
    expect(screen.getByTestId("create-pr-drafted-note")).toBeInTheDocument();
  });

  it("drops the drafted note the moment the title is edited", async () => {
    renderForm();
    await waitFor(() => expect(titleField().value).not.toBe(""));

    fireEvent.change(titleField(), { target: { value: "My own words" } });

    expect(screen.queryByTestId("create-pr-drafted-note")).not.toBeInTheDocument();
    expect(titleField().value).toBe("My own words");
  });

  it("draws the template chip only when the repository has one", async () => {
    renderForm();
    await waitFor(() => expect(titleField().value).not.toBe(""));
    expect(screen.queryByTestId("create-pr-template")).not.toBeInTheDocument();
  });

  it("offers the template when one exists, and applies it", async () => {
    templates.current = {
      "/home/dev/projects/codemux/.github/PULL_REQUEST_TEMPLATE.md": "## What changed\n",
    };
    renderForm();

    const chip = await screen.findByTestId("create-pr-template");
    fireEvent.click(chip);

    expect(bodyField().value).toContain("## What changed");
  });

  it("pushes with an upstream before creating when the branch has none", async () => {
    upstream.current = false;
    const { onCreated } = renderForm();
    await waitFor(() => expect(titleField().value).not.toBe(""));

    fireEvent.click(screen.getByTestId("create-pr-submit"));

    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(expect.objectContaining({ number: 900 })));
    expect(calls).toEqual(["push:true", "create:main:false"]);
  });

  it("pushes without setting an upstream when the branch already has one", async () => {
    renderForm();
    await waitFor(() => expect(titleField().value).not.toBe(""));

    fireEvent.click(screen.getByTestId("create-pr-submit"));

    await waitFor(() => expect(calls).toEqual(["push:false", "create:main:false"]));
  });

  it("passes the draft flag through", async () => {
    renderForm();
    await waitFor(() => expect(titleField().value).not.toBe(""));

    fireEvent.click(screen.getByRole("button", { name: "Draft" }));

    await waitFor(() => expect(calls).toContain("create:main:true"));
  });

  it("requests reviewers after the pull request exists", async () => {
    renderForm();
    await waitFor(() => expect(titleField().value).not.toBe(""));

    fireEvent.click(screen.getByTestId("create-pr-add-reviewer"));
    fireEvent.change(screen.getByLabelText("Reviewer handle"), {
      target: { value: "@juliusm" },
    });
    fireEvent.keyDown(screen.getByLabelText("Reviewer handle"), { key: "Enter" });
    fireEvent.click(screen.getByTestId("create-pr-submit"));

    await waitFor(() => expect(requestPrReview).toHaveBeenCalled());
    // Order matters: the host has nothing to attach a reviewer to until
    // the pull request is there.
    expect(calls).toEqual(["push:false", "create:main:false", "reviewer:juliusm"]);
  });

  it("keeps everything typed when the create fails, and can be retried", async () => {
    createResult.current = new Error("host unreachable");
    const { onCreated } = renderForm();
    await waitFor(() => expect(titleField().value).not.toBe(""));

    fireEvent.change(titleField(), { target: { value: "My own words" } });
    fireEvent.change(bodyField(), { target: { value: "Carefully written prose." } });
    fireEvent.click(screen.getByTestId("create-pr-submit"));

    await waitFor(() => expect(screen.getByTestId("create-pr-error")).toBeInTheDocument());
    expect(onCreated).not.toHaveBeenCalled();
    expect(titleField().value).toBe("My own words");
    expect(bodyField().value).toBe("Carefully written prose.");

    // The primary is the retry — same button, same place.
    const submit = screen.getByTestId("create-pr-submit") as HTMLButtonElement;
    expect(submit).not.toBeDisabled();
    createResult.current = PR;
    fireEvent.click(submit);
    await waitFor(() => expect(onCreated).toHaveBeenCalled());
  });

  it("warns about uncommitted work and offers the Changes pane", async () => {
    dirty.current = [{ path: "a.ts" }, { path: "b.ts" }];
    const { onOpenChanges } = renderForm();

    const warning = await screen.findByTestId("create-pr-dirty");
    expect(warning).toHaveTextContent("2 files have uncommitted changes");

    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(onOpenChanges).toHaveBeenCalled();
  });
});
