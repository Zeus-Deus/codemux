import { describe, expect, it } from "vitest";

import { ORB_FALLBACK_STATE, orbStateForTool, resolveOrbState } from "./orb-state";

describe("orbStateForTool", () => {
  it("maps file reading and repo search to searching", () => {
    for (const tool of ["Read", "Grep", "Glob", "LS", "read_file", "list_dir"]) {
      expect(orbStateForTool(tool)).toBe("searching");
    }
  });

  it("maps writes and edits to composing", () => {
    for (const tool of ["Write", "Edit", "MultiEdit", "apply_patch", "NotebookEdit"]) {
      expect(orbStateForTool(tool)).toBe("composing");
    }
  });

  it("maps shells to working", () => {
    for (const tool of ["Bash", "shell", "run_terminal_cmd"]) {
      expect(orbStateForTool(tool)).toBe("working");
    }
  });

  it("maps network and version-control tools to connecting", () => {
    for (const tool of ["WebFetch", "gh", "git"]) {
      expect(orbStateForTool(tool)).toBe("connecting");
    }
  });

  it("treats a web search as a search, not a connection", () => {
    expect(orbStateForTool("WebSearch")).toBe("searching");
    expect(orbStateForTool("web_search")).toBe("searching");
  });

  it("parks question-style tools on listening", () => {
    expect(orbStateForTool("AskUserQuestion")).toBe("listening");
    expect(orbStateForTool("ExitPlanMode")).toBe("listening");
  });

  it("is case-insensitive", () => {
    expect(orbStateForTool("GREP")).toBe("searching");
    expect(orbStateForTool("edit")).toBe("composing");
  });

  it("reads through an MCP server prefix to the tool name", () => {
    expect(orbStateForTool("mcp__codemux__git_status")).toBe("connecting");
    expect(orbStateForTool("mcp__codemux__browser_screenshot")).toBe("connecting");
    expect(orbStateForTool("mcp__some__grep_files")).toBe("searching");
  });

  it("returns null for a name that carries no signal", () => {
    expect(orbStateForTool("Frobnicate")).toBeNull();
    expect(orbStateForTool("")).toBeNull();
    expect(orbStateForTool(null)).toBeNull();
    expect(orbStateForTool(undefined)).toBeNull();
  });

  describe("shell command intent", () => {
    const shell = (command: string) => orbStateForTool("Bash", { command });

    it("reads a merge as weaving even though git is a network verb", () => {
      expect(shell("git merge --no-ff origin/main")).toBe("weaving");
      expect(shell("git rebase -i HEAD~3")).toBe("weaving");
    });

    it("reads talking to a remote as connecting", () => {
      expect(shell("git push origin HEAD")).toBe("connecting");
      expect(shell("gh pr list")).toBe("connecting");
      expect(shell("curl -s https://example.com")).toBe("connecting");
    });

    it("reads test and build runners as working", () => {
      expect(shell("npm run test")).toBe("working");
      expect(shell("cargo check --manifest-path src-tauri/Cargo.toml")).toBe(
        "working",
      );
      expect(shell("vitest run")).toBe("working");
    });

    it("reads repo-searching commands as searching", () => {
      expect(shell("rg 'AgentOrb' src/")).toBe("searching");
      expect(shell("ls -la")).toBe("searching");
    });

    it("falls back to working for an unrecognized command", () => {
      expect(shell("./scripts/frobnicate.sh")).toBe("working");
    });

    it("falls back to working when a shell has no command string", () => {
      expect(orbStateForTool("Bash", {})).toBe("working");
      expect(orbStateForTool("Bash", null)).toBe("working");
    });
  });
});

describe("resolveOrbState", () => {
  it("falls back to working when nothing is known", () => {
    expect(resolveOrbState({})).toBe(ORB_FALLBACK_STATE);
    expect(resolveOrbState({})).toBe("working");
  });

  it("maps a running tool", () => {
    expect(resolveOrbState({ toolName: "Grep" })).toBe("searching");
  });

  it("puts a blocked turn on listening ahead of whatever tool ran last", () => {
    expect(resolveOrbState({ awaitingUser: true, toolName: "Grep" })).toBe(
      "listening",
    );
  });

  it("puts queued work on breathing", () => {
    expect(resolveOrbState({ queued: true })).toBe("breathing");
  });

  it("puts a retry on solving ahead of the tool being retried", () => {
    expect(resolveOrbState({ retrying: true, toolName: "Bash" })).toBe("solving");
  });

  it("prefers the stopped reason over the moving one", () => {
    expect(resolveOrbState({ awaitingUser: true, queued: true })).toBe("listening");
    expect(resolveOrbState({ queued: true, retrying: true })).toBe("breathing");
  });
});
