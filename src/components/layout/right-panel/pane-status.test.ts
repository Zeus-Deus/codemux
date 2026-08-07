import { describe, expect, it } from "vitest";

import { deckStatusLine, type DeckStatusInput } from "./pane-status";

function input(overrides: Partial<DeckStatusInput> = {}): DeckStatusInput {
  return {
    activePane: "files",
    paneCount: 3,
    agentsWorking: 0,
    tasks: null,
    changes: null,
    review: null,
    diff: null,
    browser: null,
    ...overrides,
  };
}

describe("deckStatusLine", () => {
  // The deck lost its breadcrumb row when the panel collapsed to one band
  // of chrome, so the foot is now the only place the diff pane says which
  // file it is showing.
  it("names the diff pane's file, and says so when there isn't one", () => {
    expect(
      deckStatusLine(
        input({ activePane: "diff", diff: { filePath: "src/lib/utils.ts" } }),
      ),
    ).toBe("src/lib/utils.ts");
    expect(
      deckStatusLine(input({ activePane: "diff", diff: { filePath: null } })),
    ).toBe("no file selected");
  });

  it("follows the tasks pane's own counts", () => {
    expect(
      deckStatusLine(
        input({
          activePane: "tasks",
          tasks: { completed: 3, total: 4, working: 1 },
        }),
      ),
    ).toBe("3 of 4 done · 1 working");
  });

  it("drops the working clause when nothing is in flight", () => {
    expect(
      deckStatusLine(
        input({
          activePane: "tasks",
          tasks: { completed: 4, total: 4, working: 0 },
        }),
      ),
    ).toBe("4 of 4 done");
  });

  it("reports the working tree from the git watcher's own figures", () => {
    expect(
      deckStatusLine(
        input({
          activePane: "changes",
          changes: { changedFiles: 4, additions: 130, deletions: 12 },
        }),
      ),
    ).toBe("4 files changed · +130 −12");
  });

  it("names a clean tree instead of printing zeroes", () => {
    expect(
      deckStatusLine(
        input({
          activePane: "changes",
          changes: { changedFiles: 0, additions: 0, deletions: 0 },
        }),
      ),
    ).toBe("working tree clean");
  });

  it("says there is no pull request rather than inventing one", () => {
    expect(
      deckStatusLine(
        input({ activePane: "review", review: { prNumber: null, state: null } }),
      ),
    ).toBe("no pull request");
  });

  it("shows the PR number and state when one exists", () => {
    expect(
      deckStatusLine(
        input({ activePane: "review", review: { prNumber: 256, state: "OPEN" } }),
      ),
    ).toBe("PR #256 · open");
  });

  // Panes with no numbers of their own fall back to the deck's shape,
  // which is still real state — how many panes, how many agents busy.
  it("falls back to the deck's own shape for panes with no counts", () => {
    expect(deckStatusLine(input({ activePane: "files", agentsWorking: 1 }))).toBe(
      "3 panes · 1 agent working",
    );
  });

  it("pluralises the deck line and names an idle panel honestly", () => {
    expect(
      deckStatusLine(input({ activePane: "files", paneCount: 1, agentsWorking: 0 })),
    ).toBe("1 pane · idle");
    expect(deckStatusLine(input({ agentsWorking: 2 }))).toBe(
      "3 panes · 2 agents working",
    );
  });

  // A doc pane is a file view, not a counter — it reports the deck line.
  it("uses the deck line for doc panes", () => {
    expect(
      deckStatusLine(input({ activePane: "doc:/p/AGENTS.md", agentsWorking: 1 })),
    ).toBe("3 panes · 1 agent working");
  });

  it("shows the browser pane's address, marked as the agent's session", () => {
    expect(
      deckStatusLine(
        input({
          activePane: "browser",
          browser: {
            docked: true,
            url: "https://example.com/docs/",
            agentDriven: true,
          },
        }),
      ),
    ).toBe("example.com/docs · agent session");
  });

  it("drops the agent clause when nothing is driving the browser", () => {
    expect(
      deckStatusLine(
        input({
          activePane: "browser",
          browser: {
            docked: true,
            url: "https://example.com",
            agentDriven: false,
          },
        }),
      ),
    ).toBe("example.com");
  });

  it("says the browser is still starting before it docks", () => {
    expect(
      deckStatusLine(
        input({
          activePane: "browser",
          browser: { docked: false, url: null, agentDriven: false },
        }),
      ),
    ).toBe("starting browser…");
  });
});
