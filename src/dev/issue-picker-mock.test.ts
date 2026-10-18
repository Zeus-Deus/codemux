import { beforeAll, expect, it } from "vitest";
import type { AppStateSnapshot, GitHubIssue } from "@/tauri/types";

type Invoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;
let invoke: Invoke;
let projectPath: string;

beforeAll(async () => {
  await import("./tauri-mock");
  invoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: Invoke } }).__TAURI_INTERNALS__.invoke;
  const state = await invoke("get_app_state") as AppStateSnapshot;
  projectPath = state.workspaces.find((workspace) => workspace.workspace_id === "ws-codemux-chat")!.cwd;
});

it("lists the existing linked issues for a repository from either picker entry point", async () => {
  const byPath = await invoke("list_github_issues_by_path", { path: projectPath }) as GitHubIssue[];
  expect(byPath.map((issue) => issue.number)).toContain(146);
  expect(await invoke("list_github_issues", { workspaceId: "ws-codemux-chat" })).toEqual(byPath);
  expect(await invoke("list_github_issues_by_path", { path: "/another-repository" })).toEqual([]);
});

it("searches the fixture issues by number and title", async () => {
  const byNumber = await invoke("list_github_issues_by_path", { path: projectPath, search: "146" }) as GitHubIssue[];
  const byTitle = await invoke("list_github_issues_by_path", { path: projectPath, search: "context row" });
  expect(byNumber).toHaveLength(1);
  expect(byTitle).toEqual(byNumber);
  expect(await invoke("list_github_issues_by_path", { path: projectPath, search: "no matching issue" })).toEqual([]);
});

it("resolves a numeric issue mention using the same fixture as the picker", async () => {
  const issue = await invoke("get_github_issue_by_path", { path: projectPath, issueNumber: 146 });
  expect(issue).toEqual(await invoke("get_github_issue", { workspaceId: "ws-codemux-chat", issueNumber: 146 }));
  await expect(invoke("get_github_issue_by_path", { path: projectPath, issueNumber: 999999 })).rejects.toThrow(/not found/);
});
