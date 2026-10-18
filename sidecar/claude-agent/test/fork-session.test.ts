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
  const typed = ["fix the parser", "now add tests", "continue"];

  test("keeps everything before the first prompt left out", () => {
    expect(findCut(TRANSCRIPT, { prompts: typed, cutIndex: 1 })).toBe("i1");
    expect(findCut(TRANSCRIPT, { prompts: typed, cutIndex: 2 })).toBe("a2");
  });

  test("lines up from the end when older prompts went to another provider", () => {
    const repeated = [
      user("r1", "one"),
      assistant("ra1"),
      user("r2", "continue"),
      assistant("ra2"),
      user("r3", "continue"),
      assistant("ra3"),
    ];
    // Cutting before the first "continue" must not keep it.
    expect(findCut(repeated, { prompts: ["handed off", "one", "continue", "continue"], cutIndex: 2 })).toBe(
      "ra1",
    );
  });

  test("places a cut after a prompt Codemux expanded from a skill", () => {
    const expanded = [
      user("s1", "fix it"),
      assistant("sa1"),
      user("s2", [{ type: "text", text: "<skill name=review>…</skill>" }, { type: "text", text: "continue" }]),
      assistant("sa2"),
    ];
    expect(findCut(expanded, { prompts: ["fix it", "/review continue"], cutIndex: 1 })).toBe("sa1");
  });

  test("refuses a cut it can't place with certainty", () => {
    // The lists don't line up: a checkable prompt differs.
    expect(() => findCut(TRANSCRIPT, { prompts: ["fix the parser", "something else", "continue"], cutIndex: 2 })).toThrow(
      "fork_point_not_found",
    );
    // Claude holds prompts the chat never sent.
    expect(() => findCut(TRANSCRIPT, { prompts: ["now add tests", "continue"], cutIndex: 1 })).toThrow(
      "fork_point_not_found",
    );
    // The turn predates this Claude session.
    expect(() => findCut(TRANSCRIPT, { prompts: ["old", "fix the parser", "now add tests", "continue"], cutIndex: 0 })).toThrow(
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
      { sessionId: "s", dir: "/src", targetDir: target, cut: { prompts: ["fix the parser", "now add tests", "continue"], cutIndex: 2 } },
      d,
    );
    expect(options).toEqual({ dir: "/src", upToMessageId: "a2" });
    const moved = join(root!, "projects", projectDirName(target));
    expect(readdirSync(moved)).toEqual(["fork-1.jsonl"]);
    expect(readdirSync(join(root!, "projects", "-src"))).toEqual([]);
  });
});
