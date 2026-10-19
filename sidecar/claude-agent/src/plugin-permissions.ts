/** The CLI sees one bridge tool; its target must retain native permissions. */
import type {
  CanUseTool,
  HookCallback,
  PermissionUpdate,
} from "@anthropic-ai/claude-agent-sdk";
import { logger } from "./logger.ts";

const BRIDGE = "mcp__codemux__codemux_call_plugin_tool";
const FAILURE =
  "Codemux couldn't read Claude's permission rules for plugin tools (SDK changed); call the tool directly or update Codemux.";
type Behavior = "allow" | "deny" | "ask";
type Rules = Record<Behavior, string[]>;
const object = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
function target(
  input: unknown,
): { name: string; args: Record<string, unknown> } | null {
  if (
    !object(input) ||
    typeof input.name !== "string" ||
    !/^[a-zA-Z0-9_-]{1,50}$/.test(input.name) ||
    !object(input.arguments)
  )
    return null;
  return { name: `mcp__codemux__${input.name}`, args: input.arguments };
}
export function readEffectiveRules(value: unknown): Rules {
  if (!object(value) || !object(value.effective))
    throw new Error("getSettings response has no effective settings");
  if (value.effective.allowManagedPermissionRulesOnly === true)
    throw new Error(
      "SDK settings view does not expose filtered managed-only permission state",
    );
  const permissions = value.effective.permissions ?? {};
  if (!object(permissions))
    throw new Error("getSettings permissions changed shape");
  const result: Rules = { allow: [], deny: [], ask: [] };
  for (const behavior of ["allow", "deny", "ask"] as const) {
    const rules = permissions[behavior] ?? [];
    if (
      !Array.isArray(rules) ||
      rules.length > 8192 ||
      !rules.every(
        (rule): rule is string =>
          typeof rule === "string" && rule.length <= 4096,
      )
    )
      throw new Error(
        "getSettings permission rules changed shape or exceed limits",
      );
    result[behavior] = [...rules];
  }
  return result;
}
/** MCP settings rules match names; CLI deny specifiers may match scalar args. */
export function matchesPluginRule(
  rule: string,
  name: string,
  args: Record<string, unknown>,
  behavior: Behavior,
): boolean {
  const specifier = rule.indexOf("(");
  let tool = specifier < 0 ? rule : rule.slice(0, specifier);
  if (specifier >= 0) {
    if (behavior === "allow" || !rule.endsWith(")")) return false;
    const content = rule.slice(specifier + 1, -1);
    const colon = content.indexOf(":");
    if (colon < 1) return false;
    const value = args[content.slice(0, colon)];
    if (
      (typeof value !== "string" &&
        typeof value !== "number" &&
        typeof value !== "boolean") ||
      String(value) !== content.slice(colon + 1)
    )
      return false;
  }
  if (behavior === "allow" && tool.includes("*") && !/^mcp__[^*]+__/.test(tool))
    return false;
  if (/^mcp__[^_]+$/.test(tool)) tool += "__*";
  const pattern = tool
    .split("*")
    .map((part) => part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))
    .join(".*");
  return new RegExp(`^${pattern}$`).test(name);
}
function cliRules(
  flags: Record<string, string | null> | undefined,
  names: string[],
): string[] {
  return names.flatMap(
    (name) => flags?.[name]?.match(/(?:[^,\s(]|\([^)]*\))+/g) ?? [],
  );
}

/** Runtime seam is deliberately checked: SDK 0.2.114 implements getSettings
 * but omits it from Query's public type. The contract test pins that version. */
export async function queryPermissionSettings(
  query: unknown,
): Promise<unknown> {
  if (!object(query) || typeof query.getSettings !== "function")
    throw new Error("SDK query.getSettings is unavailable");
  return query.getSettings();
}
export function createPluginPermissions(input: {
  threadId: string;
  getQuery: () => unknown;
  getMode: () => string | undefined;
  extraArgs?: Record<string, string | null>;
  canUseTool: CanUseTool;
  canUseTarget: CanUseTool;
}) {
  let logged = false;
  const sessionRules: Rules = { allow: [], deny: [], ask: [] };
  const remember = (updates: PermissionUpdate[] | undefined) => {
    for (const update of updates ?? []) {
      if (
        update.destination !== "session" ||
        !("rules" in update) ||
        !("behavior" in update)
      )
        continue;
      const entries = update.rules.map(
        (rule) =>
          rule.toolName +
          (rule.ruleContent === undefined ? "" : `(${rule.ruleContent})`),
      );
      if (update.type === "replaceRules")
        sessionRules[update.behavior] = entries;
      else if (update.type === "addRules")
        sessionRules[update.behavior] = [
          ...new Set([...sessionRules[update.behavior], ...entries]),
        ].slice(-8192);
      else if (update.type === "removeRules")
        sessionRules[update.behavior] = sessionRules[update.behavior].filter(
          (rule) => !entries.includes(rule),
        );
    }
  };
  const hook: HookCallback = async (event, _toolUseId, _options) => {
    if (event.hook_event_name !== "PreToolUse" || event.tool_name !== BRIDGE)
      return {};
    const call = target(event.tool_input);
    let decision: "allow" | "deny" | "ask" = "deny";
    let reason =
      "Invalid plugin bridge target or arguments; list plugin tools again.";
    if (call) {
      let timer: ReturnType<typeof setTimeout> | undefined;
      try {
        const raw = await Promise.race([
          queryPermissionSettings(input.getQuery()),
          new Promise<never>((_, reject) => {
            timer = setTimeout(
              () => reject(new Error("getSettings timed out")),
              5000,
            );
          }),
        ]);
        const rules = readEffectiveRules(raw);
        rules.deny.push(
          ...cliRules(input.extraArgs, ["disallowedTools", "disallowed-tools"]),
        );
        rules.allow.push(
          ...cliRules(input.extraArgs, ["allowedTools", "allowed-tools"]),
        );
        const matches = (behavior: Behavior) =>
          [...rules[behavior], ...sessionRules[behavior]].some((rule) =>
            matchesPluginRule(rule, call.name, call.args, behavior),
          );
        if (matches("deny")) {
          decision = "deny";
          reason = `Permission to use ${call.name} has been denied by a rule.`;
        } else if (matches("ask")) {
          decision = input.getMode() === "dontAsk" ? "deny" : "ask";
          reason = `Permission required for ${call.name}.`;
        } else if (
          input.getMode() === "bypassPermissions" ||
          matches("allow")
        ) {
          decision = "allow";
          reason = `Permission granted for ${call.name}.`;
        } else {
          decision = input.getMode() === "dontAsk" ? "deny" : "ask";
          reason = `Permission required for ${call.name}.`;
        }
      } catch (error) {
        reason = FAILURE;
        if (!logged) {
          logged = true;
          logger.warn(FAILURE, {
            threadId: input.threadId,
            err: error instanceof Error ? error.message : String(error),
          });
        }
      } finally {
        if (timer !== undefined) clearTimeout(timer);
      }
    }
    return {
      hookSpecificOutput: {
        hookEventName: "PreToolUse",
        permissionDecision: decision,
        permissionDecisionReason: reason,
      },
    };
  };
  const canUseTool: CanUseTool = async (name, args, options) => {
    if (name !== BRIDGE) {
      const result = await input.canUseTool(name, args, options);
      if (result.behavior === "allow") remember(result.updatedPermissions);
      return result;
    }
    const call = target(args);
    if (!call)
      return {
        behavior: "deny",
        message:
          "Invalid plugin bridge target or arguments; list plugin tools again.",
      };
    const result = await input.canUseTarget(call.name, call.args, {
      ...options,
      title: `Use ${call.name}?`,
      displayName: call.name,
      suggestions: [
        {
          type: "addRules",
          behavior: "allow",
          destination: "session",
          rules: [{ toolName: call.name }],
        },
      ],
    });
    if (result.behavior !== "allow") return result;
    const updates = result.updatedPermissions?.map((update) =>
      "rules" in update
        ? {
            ...update,
            rules: update.rules.map((rule) => ({
              ...rule,
              toolName: call.name,
            })),
          }
        : update,
    );
    remember(updates);
    return {
      ...result,
      updatedInput: { ...args, arguments: result.updatedInput ?? call.args },
      ...(updates === undefined ? {} : { updatedPermissions: updates }),
    };
  };
  // Session rules belong to one CLI process, just like native grants.
  const resetSessionRules = () => {
    sessionRules.allow = [];
    sessionRules.deny = [];
    sessionRules.ask = [];
  };
  return { hook, canUseTool, resetSessionRules };
}
