/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mockOpenUrl = vi.fn().mockResolvedValue(undefined);
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...a: unknown[]) => mockOpenUrl(...a),
}));

vi.mock("@/lib/toast", () => ({
  toast: { info: vi.fn(), success: vi.fn(), warning: vi.fn(), error: vi.fn() },
}));

import {
  BranchLocalOnlyState,
  CliMissingState,
  NoPullRequestState,
  RepoUnreachableState,
  SignedOutState,
  UnsupportedHostState,
} from "./review-empty-states";
import { resolveProvider } from "@/lib/source-control";

const github = resolveProvider("github");
const gitlab = resolveProvider("gitlab");
const bitbucket = resolveProvider("bitbucket");

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
});

afterEach(() => cleanup());

// Five different nothings — each one a different sentence and a
// different next step. None of them is an empty panel, and none of them
// is a control that cannot work.

describe("no PR for a pushed branch", () => {
  it("says how far ahead the branch is and offers to open one", async () => {
    const user = userEvent.setup();
    const onCreate = vi.fn();
    const onViewCommits = vi.fn();
    render(
      <NoPullRequestState
        branch="drop-ports-agent-guidance"
        baseBranch="main"
        commitsAhead={4}
        provider={github}
        onCreate={onCreate}
        onViewCommits={onViewCommits}
      />,
    );

    expect(screen.getByText("No pull request yet")).toBeInTheDocument();
    expect(screen.getByTestId("empty-no-pr")).toHaveTextContent(
      "drop-ports-agent-guidance is pushed and 4 commits ahead of main.",
    );

    await user.click(screen.getByRole("button", { name: "Open a pull request" }));
    expect(onCreate).toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "View commits" }));
    expect(onViewCommits).toHaveBeenCalled();
  });

  it("uses the product's own noun", () => {
    render(
      <NoPullRequestState
        branch="feat/x"
        baseBranch="main"
        commitsAhead={1}
        provider={gitlab}
        onCreate={vi.fn()}
        onViewCommits={vi.fn()}
      />,
    );
    expect(screen.getByText("No merge request yet")).toBeInTheDocument();
    expect(screen.getByTestId("empty-no-pr")).toHaveTextContent("1 commit ahead");
  });
});

describe("branch is local only", () => {
  it("counts the uncommitted files and sends you where the work is", async () => {
    const user = userEvent.setup();
    const onCommitAndPush = vi.fn();
    render(
      <BranchLocalOnlyState
        changedFiles={2}
        pushOnly={false}
        onCommitAndPush={onCommitAndPush}
        onOpenChanges={vi.fn()}
      />,
    );

    expect(screen.getByText("Nothing pushed yet")).toBeInTheDocument();
    expect(screen.getByTestId("empty-local-only")).toHaveTextContent(
      "This branch exists only on this machine. 2 files have uncommitted changes.",
    );
    await user.click(screen.getByRole("button", { name: "Commit and push" }));
    expect(onCommitAndPush).toHaveBeenCalled();
  });

  it("says Push branch when there is nothing to commit first", () => {
    render(
      <BranchLocalOnlyState
        changedFiles={0}
        pushOnly
        onCommitAndPush={vi.fn()}
        onOpenChanges={vi.fn()}
      />,
    );
    // The label promises only what the button actually does.
    expect(screen.getByRole("button", { name: "Push branch" })).toBeInTheDocument();
    expect(screen.getByTestId("empty-local-only")).not.toHaveTextContent(
      "uncommitted changes",
    );
  });
});

describe("host not authenticated", () => {
  it("names the product and hands over the command that fixes it", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });

    render(<SignedOutState provider={github} />);

    expect(screen.getByText("Sign in to GitHub")).toBeInTheDocument();
    expect(
      screen.getByText(
        "Reviewing needs a signed-in host. Everything else in this workspace keeps working.",
      ),
    ).toBeInTheDocument();
    expect(screen.getByText("gh auth login")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Sign in" }));
    expect(writeText).toHaveBeenCalledWith("gh auth login");
  });

  it("names glab on a GitLab checkout", () => {
    render(<SignedOutState provider={gitlab} />);
    expect(screen.getByText("Sign in to GitLab")).toBeInTheDocument();
    expect(screen.getByText("glab auth login")).toBeInTheDocument();
  });
});

describe("CLI missing", () => {
  it("points at the install page rather than a login", async () => {
    const user = userEvent.setup();
    render(<CliMissingState provider={github} />);

    expect(screen.getByText("GitHub CLI (gh) isn't installed")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Install gh" }));
    // `installUrl` is stored scheme-less so it reads as prose; the
    // opener still needs a real URL.
    expect(mockOpenUrl).toHaveBeenCalledWith("https://cli.github.com");
  });
});

describe("repo unreachable", () => {
  it("names the repo, guesses why, and offers a retry", async () => {
    const user = userEvent.setup();
    const onRetry = vi.fn();
    render(
      <RepoUnreachableState
        repoSlug="acme/internal-tools"
        provider={github}
        onRetry={onRetry}
      />,
    );

    expect(screen.getByText("Can't reach this repository")).toBeInTheDocument();
    expect(screen.getByTestId("empty-unreachable")).toHaveTextContent(
      "Your account can't see acme/internal-tools. It may be private, or the token may lack repo scope.",
    );
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalled();
  });
});

describe("host not supported", () => {
  it("is a sentence and the browser, never a disabled control", async () => {
    const user = userEvent.setup();
    render(
      <UnsupportedHostState
        provider={bitbucket}
        url="https://bitbucket.org/acme/x/pull-requests/4"
      />,
    );

    expect(screen.getByText("Bitbucket, read-only")).toBeInTheDocument();
    expect(
      screen.getByText(
        "Codemux can see this repository but can't review or merge it here yet.",
      ),
    ).toBeInTheDocument();
    // Binding rule 5: no greyed-out control anywhere in this state.
    for (const button of screen.getAllByRole("button")) {
      expect(button).not.toBeDisabled();
    }

    await user.click(screen.getByRole("button", { name: "Open in browser" }));
    expect(mockOpenUrl).toHaveBeenCalledWith(
      "https://bitbucket.org/acme/x/pull-requests/4",
    );
  });

  it("does not borrow a vendor's name for a repo that vendor doesn't serve", () => {
    // GitHub's presentation with an unserved checkout: naming GitHub
    // here would be a lie about why the panel is empty.
    render(<UnsupportedHostState provider={github} url={null} />);
    expect(
      screen.getByText("No supported source control host for this repository"),
    ).toBeInTheDocument();
    expect(screen.queryByText(/GitHub, read-only/)).not.toBeInTheDocument();
  });
});
