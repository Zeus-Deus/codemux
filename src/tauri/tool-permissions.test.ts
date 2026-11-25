import { expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));

import { invoke } from "@tauri-apps/api/core";
import { listToolPermissions } from "./commands";

it("requests scope errors alongside available permission rules", async () => {
  const result = {
    rules: [],
    scope_errors: [{ scope: "project", message: "Project settings unavailable." }],
  };
  vi.mocked(invoke).mockResolvedValueOnce(result);

  expect(await listToolPermissions("/workspace/sample")).toEqual(result);
  expect(invoke).toHaveBeenLastCalledWith("list_tool_permissions", {
    projectRoot: "/workspace/sample",
    includeScopeErrors: true,
  });
});

it("accepts the legacy array returned by an older host", async () => {
  const rules = [{
    tool_name: "Read",
    rule_content: null,
    behavior: "allow",
    scope: "user",
    source_path: "/home/example/.claude/settings.json",
  }];
  vi.mocked(invoke).mockResolvedValueOnce(rules);

  expect(await listToolPermissions(null)).toEqual({ rules, scope_errors: [] });
});
