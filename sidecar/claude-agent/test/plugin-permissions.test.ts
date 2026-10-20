import { describe, test, expect, spyOn } from "bun:test";
import { copyFile, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
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
  test.each([false, true])("pending approvals survive catalog-call pressure (expired: %s)", async (expired) => {
    const clock = spyOn(Date, "now").mockReturnValue(1_000);
    let approve!: (result: PermissionResult) => void;
    let prompt!: () => void;
    const prompted = new Promise<void>((resolve) => {
      prompt = resolve;
    });
    const gate = createPluginPermissions({
      threadId: "fixture",
      getQuery: () => ({
        getSettings: async () => ({ effective: { permissions: {} } }),
      }),
      getMode: () => "default",
      canUseTool: async () => ({ behavior: "deny", message: "unused" }),
      canUseTarget: () => {
        prompt();
        return new Promise((resolve) => {
          approve = resolve;
        });
      },
    });
    try {
      await decision(gate, "x");
      const pending = gate.canUseTool(
        bridge,
        { name: "fixture__x", arguments: { value: 7 } },
        { toolUseID: "call", signal: new AbortController().signal },
      );
      await prompted;
      if (expired) clock.mockReturnValue(11 * 60_000);
      for (let i = 0; i < 129; i++) {
        await gate.hook(
          { ...hookInput("y"), tool_use_id: `other-${i}` },
          `other-${i}`,
          { signal: new AbortController().signal },
        );
      }
      approve({ behavior: "allow" });
      expect(await pending).toMatchObject({ behavior: "allow" });
      await expect(gate.authorizeCall(
        { name: "fixture__x", arguments: { value: 7 } },
        { _meta: { "claudecode/toolUseId": "call" } },
      )).resolves.toBeUndefined();
    } finally {
      clock.mockRestore();
    }
  });
  test("an invalidated approval has its own actionable message", async () => {
    let approve!: (result: PermissionResult) => void;
    let prompt!: () => void;
    const prompted = new Promise<void>((resolve) => {
      prompt = resolve;
    });
    const gate = createPluginPermissions({
      threadId: "fixture",
      getQuery: () => ({
        getSettings: async () => ({ effective: { permissions: {} } }),
      }),
      getMode: () => "default",
      canUseTool: async () => ({ behavior: "deny", message: "unused" }),
      canUseTarget: () => {
        prompt();
        return new Promise((resolve) => {
          approve = resolve;
        });
      },
    });
    await decision(gate, "x");
    const pending = gate.canUseTool(
      bridge,
      { name: "fixture__x", arguments: { value: 7 } },
      { toolUseID: "call", signal: new AbortController().signal },
    );
    await prompted;
    gate.clearTickets();
    approve({ behavior: "allow" });
    expect(await pending).toEqual({
      behavior: "deny",
      message: "This approval expired; ask the agent to run the tool again.",
    });
  });
  test("round 4: missing permissions fails closed even in bypass", async () => {
    const { gate, settings } = setup("bypassPermissions");
    settings({ effective: {} });
    expect(await decision(gate, "x")).toMatchObject({
      permissionDecision: "deny",
    });
  });
  test("round 4: canUseTool denies a target rule without prompting", async () => {
    const { gate, calls } = setup("default", { deny: [target("x")] });
    const result = await gate.canUseTool(
      bridge,
      { name: "fixture__x", arguments: {} },
      { toolUseID: "call", signal: new AbortController().signal },
    );
    expect(result.behavior).toBe("deny");
    expect(calls).toEqual([]);
  });
  test("round 4: handler tickets bind exact call ID and arguments, once only", async () => {
    const { gate } = setup("bypassPermissions");
    const args = { name: "fixture__x", arguments: { value: 7 } };
    const extra = { _meta: { "claudecode/toolUseId": "call" } };
    await expect(gate.authorizeCall(args, extra)).rejects.toThrow(
      "permission check",
    );
    await decision(gate, "x");
    await expect(
      gate.authorizeCall(args, { _meta: { "claudecode/toolUseId": "other" } }),
    ).rejects.toThrow("permission check");
    await expect(
      gate.authorizeCall({ ...args, arguments: { value: 8 } }, extra),
    ).rejects.toThrow("permission check");
    await expect(gate.authorizeCall(args, extra)).resolves.toBeUndefined();
    await expect(gate.authorizeCall(args, extra)).rejects.toThrow(
      "permission check",
    );
  });
  test("round 4: a hook still reading settings cannot authorize after Stop", async () => {
    let finish!: (value: unknown) => void;
    const pending = new Promise((resolve) => {
      finish = resolve;
    });
    const gate = createPluginPermissions({
      threadId: "fixture",
      getQuery: () => ({ getSettings: () => pending }),
      getMode: () => "bypassPermissions",
      canUseTool: async () => ({ behavior: "allow" }),
      canUseTarget: async () => ({ behavior: "allow" }),
    });
    const check = gate.hook(hookInput("x"), "call", {
      signal: new AbortController().signal,
    });
    gate.clearTickets();
    finish({ effective: { permissions: {} } });
    expect(await check).toMatchObject({
      hookSpecificOutput: { permissionDecision: "deny" },
    });
  });
  test("round 4: an ask ticket cannot execute before the target is approved", async () => {
    const { gate } = setup();
    await decision(gate, "x");
    await expect(
      gate.authorizeCall(
        { name: "fixture__x", arguments: { value: 7 } },
        { _meta: { "claudecode/toolUseId": "call" } },
      ),
    ).rejects.toThrow("permission check");
  });
  test("round 4: omitted arguments mean an empty object on every permission path", async () => {
    const { gate } = setup("bypassPermissions");
    const event = hookInput("x");
    event.tool_input = { name: "fixture__x" };
    expect(
      await gate.hook(event, "call", { signal: new AbortController().signal }),
    ).toMatchObject({ hookSpecificOutput: { permissionDecision: "allow" } });
    await expect(
      gate.authorizeCall(
        { name: "fixture__x" },
        { _meta: { "claudecode/toolUseId": "call" } },
      ),
    ).resolves.toBeUndefined();
  });
  test("round 4: the hook's actual mode wins over the app's earlier mode", async () => {
    const { gate } = setup("bypassPermissions");
    const result = await gate.hook(
      { ...hookInput("x"), permission_mode: "plan" },
      "call",
      { signal: new AbortController().signal },
    );
    expect(result).toMatchObject({
      hookSpecificOutput: { permissionDecision: "ask" },
    });
  });
  test("round 4: catalog filtering removes denied names from text and structured data", async () => {
    const { gate } = setup("default", { deny: [target("x")] });
    const data = { tools: [{ name: "fixture__x" }, { name: "fixture__y" }] };
    const result = await gate.filterList({
      content: [{ type: "text", text: JSON.stringify(data) }],
      structuredContent: data,
    });
    expect(result).toMatchObject({
      structuredContent: { tools: [{ name: "fixture__y" }] },
    });
    expect(JSON.stringify(result)).not.toContain("fixture__x");
  });
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
    await decision(gate, "x");
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
    ).toMatchObject({ permissionDecision: "ask" });
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
  const root = await mkdtemp(join(tmpdir(), "codemux-sdk-settings-"));
  const fixture = pathToFileURL(join(root, "settings fixture.mjs"));
  const prompts = new AsyncPromptQueue<
    import("@anthropic-ai/claude-agent-sdk").SDKUserMessage
  >();
  let q: ReturnType<typeof query> | undefined;
  try {
    await copyFile(new URL("./fixtures/settings-cli.mjs", import.meta.url), fixture);
    q = query({
      prompt: prompts,
      options: {
        // pathname leaves /C:/ on Windows and percent-escapes spaces.
        pathToClaudeCodeExecutable: fileURLToPath(fixture),
        executable: "node",
        settingSources: [],
        env: {
          PATH: process.env.PATH,
          SystemRoot: process.env.SystemRoot,
          HOME: root,
          USERPROFILE: root,
          CLAUDE_CONFIG_DIR: root,
        },
      },
    });
    await q.initializationResult();
    const raw = await queryPermissionSettings(q);
    expect(readEffectiveRules(raw)).toEqual({
      allow: [target("allowed")],
      deny: [target("blocked")],
      ask: [],
    });
  } finally {
    prompts.close();
    q?.close();
    await rm(root, { recursive: true, force: true });
  }
});

test("a cancelled permission hook cannot leave a ticket or block catalog refresh", async () => {
  let settings!: (value: unknown) => void;
  const gate = createPluginPermissions({
    threadId: "cancelled-hook",
    getQuery: () => ({
      getSettings: () =>
        new Promise((resolve) => {
          settings = resolve;
        }),
    }),
    getMode: () => "bypassPermissions",
    canUseTool: async () => ({ behavior: "allow" }),
    canUseTarget: async () => ({ behavior: "allow" }),
  });
  const controller = new AbortController();
  const check = gate.hook(hookInput("x"), "call", {
    signal: controller.signal,
  });
  expect(gate.isBusy()).toBe(true);
  controller.abort();
  settings({ effective: { permissions: {} } });
  expect(await check).toMatchObject({
    hookSpecificOutput: { permissionDecision: "deny" },
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(gate.isBusy()).toBe(false);
  await expect(
    gate.authorizeCall(
      { name: "fixture__x", arguments: { value: 7 } },
      { _meta: { "claudecode/toolUseId": "call" } },
    ),
  ).rejects.toThrow("permission check");
});
