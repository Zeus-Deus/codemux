// Tests for `fork-session`: finding the cut in the SDK transcript and
// moving a fork into the project directory of the folder it will run in.

import { afterEach, describe, expect, test } from "bun:test";
import type { SessionMessage } from "@anthropic-ai/claude-agent-sdk";
import { mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  findCut,
  forkClaudeSession,
  projectDirName,
  type ForkSessionDeps,
} from "../src/fork-session.ts";

const user = (uuid: string, content: unknown): SessionMessage => ({
  type: "user",
  uuid,
  session_id: "s",
  message: { role: "user", content },
  parent_tool_use_id: null,
});
const assistant = (uuid: string): SessionMessage => ({
  type: "assistant",
  uuid,
  session_id: "s",
  message: { role: "assistant", content: [{ type: "text", text: "ok" }] },
  parent_tool_use_id: null,
});

const TRANSCRIPT = [
  user("u1", "fix the parser"),
  assistant("a1"),
  user("t1", [{ type: "tool_result", tool_use_id: "x", content: "done" }]),
  assistant("a1b"),
  user("i1", [{ type: "text", text: "[Request interrupted by user]" }]),
  user("u2", [{ type: "text", text: "=== Attached context ===\n…\n=== End context ===" }, { type: "text", text: "now  add tests" }]),
  assistant("a2"),
  user("u3", "continue"),
  assistant("a3"),
];

describe("findCut", () => {
  test("keeps everything before the first prompt left out", () => {
    expect(findCut(TRANSCRIPT, { keepPrompts: 1, nextPrompt: "now add tests", occurrence: 0 })).toBe("i1");
    expect(findCut(TRANSCRIPT, { keepPrompts: 2, nextPrompt: "continue", occurrence: 0 })).toBe("a2");
  });

  test("falls back to the prompt's text when the counts disagree", () => {
    // Codemux saw an extra prompt the SDK never received.
    expect(findCut(TRANSCRIPT, { keepPrompts: 3, nextPrompt: "continue", occurrence: 0 })).toBe("a2");
  });

  test("refuses a cut it can't find instead of guessing", () => {
    expect(() => findCut(TRANSCRIPT, { keepPrompts: 2, nextPrompt: "something else", occurrence: 0 })).toThrow(
      "fork_point_not_found",
    );
    expect(() => findCut(TRANSCRIPT, { keepPrompts: 0, nextPrompt: "fix the parser", occurrence: 0 })).toThrow(
      "fork_point_not_found",
    );
  });
});

describe("projectDirName", () => {
  test("matches the CLI's folder slug", () => {
    expect(projectDirName("/home/user/my.repo")).toBe("-home-user-my-repo");
    const long = `/${"a".repeat(250)}`;
    expect(projectDirName(long).startsWith(`-${"a".repeat(199)}-`)).toBe(true);
    expect(projectDirName(long).length).toBeGreaterThan(200);
  });
});

describe("forkClaudeSession", () => {
  let root: string | undefined;
  afterEach(() => {
    if (root) rmSync(root, { recursive: true, force: true });
    root = undefined;
  });

  function deps(onFork: (options: unknown) => void): ForkSessionDeps {
    root = mkdtempSync(join(tmpdir(), "codemux-fork-"));
    const config = root;
    return {
      configDir: () => config,
      getSessionMessages: async () => TRANSCRIPT,
      forkSession: async (_id, options) => {
        onFork(options);
        const dir = join(config, "projects", "-src");
        mkdirSync(dir, { recursive: true });
        writeFileSync(join(dir, "fork-1.jsonl"), "{}\n");
        return { sessionId: "fork-1" };
      },
    };
  }

  test("forks the whole session in place", async () => {
    let options: unknown;
    const d = deps((o) => (options = o));
    const result = await forkClaudeSession({ sessionId: "s", dir: "/src", targetDir: "/src" }, d);
    expect(result).toEqual({ sessionId: "fork-1" });
    expect(options).toEqual({ dir: "/src" });
  });

  test("cuts at an earlier turn and moves the fork to its new folder", async () => {
    let options: unknown;
    const d = deps((o) => (options = o));
    const target = join(root!, "worktree");
    mkdirSync(target);
    await forkClaudeSession(
      { sessionId: "s", dir: "/src", targetDir: target, cut: { keepPrompts: 2, nextPrompt: "continue", occurrence: 0 } },
      d,
    );
    expect(options).toEqual({ dir: "/src", upToMessageId: "a2" });
    const moved = join(root!, "projects", projectDirName(target));
    expect(readdirSync(moved)).toEqual(["fork-1.jsonl"]);
    expect(readdirSync(join(root!, "projects", "-src"))).toEqual([]);
  });
});
