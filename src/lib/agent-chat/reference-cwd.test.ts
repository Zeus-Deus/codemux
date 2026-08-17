import { describe, expect, it } from "vitest";

import type { ChatViewItem } from "./types";
import {
  assistantReferenceCwds,
  assistantReferencePaths,
  toolCallReferenceCwd,
  toolCallReferencePaths,
} from "./reference-cwd";

describe("toolCallReferenceCwd", () => {
  it("reads a structured exec working directory", () => {
    expect(
      toolCallReferenceCwd({
        cmd: "git status --short",
        workdir: "/home/me/projects/codemux",
      }),
    ).toBe("/home/me/projects/codemux");
  });

  it("reads a nested freeform exec working directory", () => {
    expect(
      toolCallReferenceCwd(
        {
          source:
            'const result = await tools.exec_command({"cmd":"git status","workdir":"/home/me/projects/codemux"});',
        },
      ),
    ).toBe("/home/me/projects/codemux");
  });

  it("ignores relative directories", () => {
    expect(toolCallReferenceCwd({ cwd: "projects/codemux" })).toBeNull();
  });

  it("ignores a working directory spelled inside written file content", () => {
    expect(
      toolCallReferenceCwd({
        file_path: "/home/me/projects/codemux/.vscode/launch.json",
        content: '{\n  "configurations": [{ "cwd": "/tmp/evil" }]\n}\n',
      }),
    ).toBeNull();
  });

  it("ignores a working directory quoted inside a subagent prompt", () => {
    expect(
      toolCallReferenceCwd({
        description: "run the checks",
        prompt: 'Run the suite with {"workdir":"/tmp/evil"} and report back.',
      }),
    ).toBeNull();
  });

  it("still reads the call's own directory when content mentions another", () => {
    expect(
      toolCallReferenceCwd({
        cwd: "/home/me/projects/codemux",
        content: '{ "cwd": "/tmp/evil" }',
      }),
    ).toBe("/home/me/projects/codemux");
  });
});

describe("toolCallReferencePaths", () => {
  it("extracts absolute file paths embedded in a command line", () => {
    expect(
      toolCallReferencePaths({
        command:
          "codemux browser screenshot /tmp/review-spec/screenshots/polish-a-page-summary.png --full-page",
      }),
    ).toEqual(["/tmp/review-spec/screenshots/polish-a-page-summary.png"]);
  });

  it("collects from nested structured input and dedupes, newest last", () => {
    expect(
      toolCallReferencePaths({
        file_path: "/tmp/shot.png",
        args: ["cp /tmp/shot.png /home/me/report/final-shot.png"],
      }),
    ).toEqual(["/tmp/shot.png", "/home/me/report/final-shot.png"]);
  });

  it("recognises Windows drive paths", () => {
    expect(
      toolCallReferencePaths({ command: String.raw`type C:\temp\out.png` }),
    ).toEqual([String.raw`C:\temp\out.png`]);
  });

  it("ignores relative paths and extensionless tokens", () => {
    expect(
      toolCallReferencePaths({
        command: "cat notes/todo.txt && ls /usr/bin/env",
      }),
    ).toEqual([]);
  });
});

describe("assistantReferenceCwds", () => {
  const user = (id: string, seq: number): ChatViewItem =>
    ({ kind: "user_message", id, seq, text: "go" }) as ChatViewItem;
  const tool = (id: string, seq: number, workdir: string): ChatViewItem =>
    ({
      kind: "tool_call",
      id,
      seq,
      tool_use_id: id,
      tool_name: "exec_command",
      input: { cmd: "git status", workdir },
      status: "done",
      result_content: "",
      approval_request_id: null,
    }) as ChatViewItem;
  const assistant = (id: string, seq: number): ChatViewItem =>
    ({ kind: "assistant_message", id, seq, text: "Updated `AGENTS.md`.", streaming: false }) as ChatViewItem;

  it("uses the latest tool directory for later prose in the same turn", () => {
    const result = assistantReferenceCwds([
      user("u1", 1),
      tool("t1", 2, "/home/me/projects/first"),
      tool("t2", 3, "/home/me/projects/second"),
      assistant("a1", 4),
    ]);

    expect(result.get("a1")).toBe("/home/me/projects/second");
  });

  it("does not leak a previous turn's directory into the next answer", () => {
    const result = assistantReferenceCwds([
      user("u1", 1),
      tool("t1", 2, "/home/me/projects/first"),
      assistant("a1", 3),
      user("u2", 4),
      assistant("a2", 5),
    ]);

    expect(result.get("a1")).toBe("/home/me/projects/first");
    expect(result.has("a2")).toBe(false);
  });

  it("ignores a tool call the user denied", () => {
    const gated = {
      ...(tool("t2", 3, "/home/me/projects/denied") as ChatViewItem & {
        approval_request_id: string | null;
      }),
      status: "error",
      approval_request_id: "req-1",
    } as ChatViewItem;
    const request = {
      kind: "permission_request",
      id: "p1",
      seq: 4,
      request_id: "req-1",
      turn_id: null,
      request_kind: "tool",
      payload: null,
      tool_use_id: "t2",
      resolution: { state: "resolved", decision: { decision: "deny", message: "no" } },
    } as ChatViewItem;

    const result = assistantReferenceCwds([
      user("u1", 1),
      tool("t1", 2, "/home/me/projects/first"),
      gated,
      request,
      assistant("a1", 5),
    ]);

    expect(result.get("a1")).toBe("/home/me/projects/first");
  });

  it("returns the previous map when nothing changed", () => {
    const items = [
      user("u1", 1),
      tool("t1", 2, "/home/me/projects/first"),
      assistant("a1", 3),
    ];
    const first = assistantReferenceCwds(items);
    const second = assistantReferenceCwds([...items], first);

    expect(second).toBe(first);
  });
});

describe("assistantReferencePaths", () => {
  const user = (id: string, seq: number): ChatViewItem =>
    ({ kind: "user_message", id, seq, text: "go" }) as ChatViewItem;
  const tool = (id: string, seq: number, command: string): ChatViewItem =>
    ({
      kind: "tool_call",
      id,
      seq,
      tool_use_id: id,
      tool_name: "Bash",
      input: { command },
      status: "done",
      result_content: "",
      approval_request_id: null,
    }) as ChatViewItem;
  const assistant = (id: string, seq: number): ChatViewItem =>
    ({
      kind: "assistant_message",
      id,
      seq,
      text: "Compare `shot.png` against the spec.",
      streaming: false,
    }) as ChatViewItem;

  it("accumulates paths across a turn's tool calls, newest last", () => {
    const result = assistantReferencePaths([
      user("u1", 1),
      tool("t1", 2, "codemux browser screenshot /tmp/spec/shot.png"),
      tool("t2", 3, "cp /tmp/spec/shot.png /tmp/spec/final.png"),
      assistant("a1", 4),
    ]);

    expect(result.get("a1")).toEqual(["/tmp/spec/shot.png", "/tmp/spec/final.png"]);
  });

  it("does not leak a previous turn's paths into the next answer", () => {
    const result = assistantReferencePaths([
      user("u1", 1),
      tool("t1", 2, "ls /tmp/spec/shot.png"),
      assistant("a1", 3),
      user("u2", 4),
      assistant("a2", 5),
    ]);

    expect(result.get("a1")).toEqual(["/tmp/spec/shot.png"]);
    expect(result.has("a2")).toBe(false);
  });

  it("keeps array identity for unchanged messages across recomputes", () => {
    const items = [
      user("u1", 1),
      tool("t1", 2, "codemux browser screenshot /tmp/spec/shot.png"),
      assistant("a1", 3),
    ];
    const first = assistantReferencePaths(items);
    // A streaming delta rebuilds the transcript array and replaces the live
    // assistant item with a new object of the same id; the reference map and
    // every row's array must keep their identity so the memoized rows skip.
    const second = assistantReferencePaths(
      [items[0], items[1], assistant("a1", 3)],
      first,
    );

    expect(second).toBe(first);
    expect(second.get("a1")).toBe(first.get("a1"));
  });

  it("reuses unchanged rows when a later message appends paths", () => {
    const items = [
      user("u1", 1),
      tool("t1", 2, "ls /tmp/spec/shot.png"),
      assistant("a1", 3),
    ];
    const first = assistantReferencePaths(items);
    const grown = assistantReferencePaths(
      [...items, tool("t2", 4, "ls /tmp/spec/next.png"), assistant("a2", 5)],
      first,
    );

    expect(grown).not.toBe(first);
    expect(grown.get("a1")).toBe(first.get("a1"));
    expect(grown.get("a2")).toEqual(["/tmp/spec/shot.png", "/tmp/spec/next.png"]);
  });
});
