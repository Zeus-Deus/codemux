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
});
