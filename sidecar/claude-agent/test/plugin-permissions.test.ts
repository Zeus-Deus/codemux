import { describe, test, expect, spyOn } from "bun:test";
import { readFile } from "node:fs/promises";
import {
  query,
  type CanUseTool,
  type PreToolUseHookInput,
  type PermissionResult,
  type PermissionUpdate,
} from "@anthropic-ai/claude-agent-sdk";
import { AsyncPromptQueue } from "../src/async-queue.ts";
import { logger } from "../src/logger.ts";
import {
  createPluginPermissions,
  readEffectiveRules,
  queryPermissionSettings,
  matchesPluginRule,
} from "../src/plugin-permissions.ts";
const bridge = "mcp__codemux__codemux_call_plugin_tool";
const target = (name: string) => `mcp__codemux__fixture__${name}`;
const hookInput = (
  name: string,
  args: Record<string, unknown> = { value: 7 },
): PreToolUseHookInput => ({
  hook_event_name: "PreToolUse",
  tool_name: bridge,
  tool_input: { name: `fixture__${name}`, arguments: args },
  tool_use_id: "call",
  session_id: "session",
  transcript_path: "/fixture",
  cwd: "/fixture",
});
function setup(mode = "default", permissions: Record<string, string[]> = {}) {
  const calls: { name: string; args: Record<string, unknown> }[] = [];
  let decision: PermissionResult = {
    behavior: "allow",
    updatedInput: { value: 8 },
  };
  let settings: unknown = { effective: { permissions } };
  const callback: CanUseTool = async (name, args) => {
    calls.push({ name, args });
    return decision;
  };
  const gate = createPluginPermissions({
    threadId: "fixture",
    getQuery: () => ({ getSettings: async () => settings }),
    getMode: () => mode,
    canUseTool: callback,
    canUseTarget: callback,
  });
  return {
    gate,
    calls,
    decide: (next: PermissionResult) => {
      decision = next;
    },
    settings: (next: unknown) => {
      settings = next;
    },
  };
}
const decision = async (
  gate: ReturnType<typeof createPluginPermissions>,
  name: string,
) => {
  const result = await gate.hook(hookInput(name), "call", {
    signal: new AbortController().signal,
  });
  return "hookSpecificOutput" in result ? result.hookSpecificOutput : undefined;
};
describe("Claude plugin bridge permissions", () => {
  test("native deny wins over bridge and target allows, even in full access", async () => {
    for (const mode of ["default", "plan", "bypassPermissions"]) {
      const { gate } = setup(mode, {
        deny: [target("blocked")],
        allow: [bridge, target("blocked")],
      });
      expect(await decision(gate, "blocked")).toMatchObject({
        permissionDecision: "deny",
        permissionDecisionReason: expect.stringContaining(target("blocked")),
      });
    }
  });
  test("an existing bridge allow does not approve its target", async () => {
    const { gate } = setup("default", { allow: [bridge] });
    expect(await decision(gate, "other")).toMatchObject({
      permissionDecision: "ask",
    });
  });
  test("approval shows target name and args, and always-allow X does not allow Y", async () => {
    const { gate, calls, decide } = setup();
    const updates: PermissionUpdate[] = [
      {
        type: "addRules",
        behavior: "allow",
        destination: "session",
        rules: [{ toolName: bridge }],
      },
    ];
    decide({
      behavior: "allow",
      updatedInput: { value: 99 },
      updatedPermissions: updates,
    });
    const result = await gate.canUseTool(
      bridge,
      { name: "fixture__x", arguments: { value: 7 } },
      { toolUseID: "call", signal: new AbortController().signal },
    );
    expect(calls).toEqual([{ name: target("x"), args: { value: 7 } }]);
    expect(result).toMatchObject({
      updatedInput: { name: "fixture__x", arguments: { value: 99 } },
      updatedPermissions: [{ rules: [{ toolName: target("x") }] }],
    });
    expect(await decision(gate, "x")).toMatchObject({
      permissionDecision: "allow",
    });
    expect(await decision(gate, "y")).toMatchObject({
      permissionDecision: "ask",
    });
  });
  test("mode semantics preserve explicit asks; plan does not autoallow unknown plugin writes", async () => {
    expect(await decision(setup("bypassPermissions").gate, "x")).toMatchObject({
      permissionDecision: "allow",
    });
    expect(
      await decision(
        setup("bypassPermissions", { ask: [target("x")] }).gate,
        "x",
      ),
    ).toMatchObject({ permissionDecision: "ask" });
    expect(await decision(setup("plan").gate, "x")).toMatchObject({
      permissionDecision: "ask",
    });
    expect(
      await decision(setup("plan", { allow: [target("x")] }).gate, "x"),
    ).toMatchObject({ permissionDecision: "allow" });
    expect(await decision(setup("dontAsk").gate, "x")).toMatchObject({
      permissionDecision: "deny",
    });
  });
  test("missing or changed SDK settings fail closed with an explicit reason", async () => {
    for (const getQuery of [
      () => ({}),
      () => ({
        getSettings: async () => {
          throw new Error("changed SDK");
        },
      }),
      () => ({ getSettings: async () => ({ changed: {} }) }),
    ]) {
      const gate = createPluginPermissions({
        threadId: "fixture",
        getQuery,
        getMode: () => "bypassPermissions",
        canUseTool: async () => ({ behavior: "deny", message: "unused" }),
        canUseTarget: async () => ({ behavior: "deny", message: "unused" }),
      });
      expect(await decision(gate, "x")).toMatchObject({
        permissionDecision: "deny",
        permissionDecisionReason: expect.stringContaining(
          "call the tool directly or update Codemux",
        ),
      });
    }
  });
  test("MCP name globs and CLI scalar deny rules match the target", () => {
    expect(
      matchesPluginRule("mcp__codemux__fixture__*", target("x"), {}, "allow"),
    ).toBe(true);
    expect(matchesPluginRule("mcp__codemux", target("x"), {}, "deny")).toBe(
      true,
    );
    expect(matchesPluginRule("*", target("x"), {}, "allow")).toBe(false);
    expect(
      matchesPluginRule(
        `${target("x")}(mode:delete)`,
        target("x"),
        { mode: "delete" },
        "deny",
      ),
    ).toBe(true);
    expect(
      matchesPluginRule(
        `${target("x")}(mode:delete)`,
        target("x"),
        { mode: "read" },
        "deny",
      ),
    ).toBe(false);
  });
  test("an unavailable settings method logs once across query rebuilds", async () => {
    const warning = spyOn(logger, "warn").mockImplementation(() => {});
    const gate = createPluginPermissions({
      threadId: "fixture",
      getQuery: () => ({}),
      getMode: () => "default",
      canUseTool: async () => ({ behavior: "deny", message: "unused" }),
      canUseTarget: async () => ({ behavior: "deny", message: "unused" }),
    });
    try {
      await decision(gate, "x");
      gate.resetSessionRules();
      await decision(gate, "y");
      expect(warning).toHaveBeenCalledTimes(1);
    } finally {
      warning.mockRestore();
    }
  });
});
test("pinned SDK 0.2.114 getSettings control contract (review this test on SDK bumps)", async () => {
  const entry = import.meta.resolve("@anthropic-ai/claude-agent-sdk");
  const pkg = JSON.parse(
    await readFile(new URL("package.json", entry), "utf8"),
  );
  expect(pkg.version).toBe("0.2.114");
  const prompts = new AsyncPromptQueue<
    import("@anthropic-ai/claude-agent-sdk").SDKUserMessage
  >();
  const q = query({
    prompt: prompts,
    options: {
      pathToClaudeCodeExecutable: new URL(
        "./fixtures/settings-cli.mjs",
        import.meta.url,
      ).pathname,
      executable: "node",
      settingSources: [],
    },
  });
  try {
    await q.initializationResult();
    const raw = await queryPermissionSettings(q);
    expect(readEffectiveRules(raw)).toEqual({
      allow: [target("allowed")],
      deny: [target("blocked")],
      ask: [],
    });
  } finally {
    prompts.close();
    q.close();
  }
});
