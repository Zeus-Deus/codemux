/** The CLI sees one bridge tool; its target must retain native permissions. */
import type {
  CanUseTool,
  HookCallback,
  PermissionUpdate,
} from "@anthropic-ai/claude-agent-sdk";
import { logger } from "./logger.ts";
import { createHash } from "node:crypto";

export class PluginPermissionError extends Error {}

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
    (input.arguments !== undefined && !object(input.arguments))
  )
    return null;
  return { name: `mcp__codemux__${input.name}`, args: input.arguments ?? {} };
}
export function readEffectiveRules(value: unknown): Rules {
  if (!object(value) || !object(value.effective))
    throw new PluginPermissionError(
      "getSettings response has no effective settings",
    );
  if (value.effective.allowManagedPermissionRulesOnly === true)
    throw new PluginPermissionError(
      "SDK settings view does not expose filtered managed-only permission state",
    );
  const permissions = value.effective.permissions;
  if (!object(permissions))
    throw new PluginPermissionError("getSettings permissions changed shape");
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
      throw new PluginPermissionError(
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
    throw new PluginPermissionError("SDK query.getSettings is unavailable");
  return query.getSettings();
}
export interface PluginBridgeGuard {
  /** Includes native calls through the shared server, not only bridge calls. */
  beginCall: () => () => void;
  authorizeCall: (args: unknown, extra: unknown) => Promise<void>;
  filterList: (result: unknown) => Promise<unknown>;
}
const NO_CHECK =
  "Codemux couldn't verify Claude's permission check for this plugin tool. Hooks may be disabled or this workspace may be untrusted; call the tool directly or enable hooks.";
const APPROVAL_EXPIRED =
  "This approval expired; ask the agent to run the tool again.";
function fingerprint(input: unknown): string {
  const json = JSON.stringify(input, (_key, value: unknown) =>
    object(value)
      ? Object.fromEntries(
          Object.entries(value).sort(([a], [b]) => a.localeCompare(b)),
        )
      : value,
  );
  if (json === undefined || Buffer.byteLength(json) > 1024 * 1024)
    throw new PluginPermissionError(
      "Plugin tool arguments exceed the permission-check limit (1 MiB)",
    );
  return createHash("sha256").update(json).digest("hex");
}
export function createPluginPermissions(input: {
  threadId: string;
  getQuery: () => unknown;
  getMode: () => string | undefined;
  extraArgs?: Record<string, string | null>;
  canUseTool: CanUseTool;
  canUseTarget: CanUseTool;
  onIdle?: () => void;
}) {
  let logged = false;
  let generation = 0;
  let active = 0;
  const changed = () => queueMicrotask(() => input.onIdle?.());
  const beginCall = () => {
    const started = generation;
    active++;
    let ended = false;
    return () => {
      if (ended) return;
      ended = true;
      // The SDK forwards a handler's result in promise continuations. Keep
      // the server busy through those continuations so a control RPC cannot
      // remove it ahead of its response (covered against the installed SDK).
      setTimeout(() => {
        if (generation === started) active--;
        changed();
      }, 0);
    };
  };
  const sessionRules: Rules = { allow: [], deny: [], ask: [] };
  // The real CLI forwards this call ID in MCP's claudecode/toolUseId meta.
  // No ticket or arguments are added to the model-facing tool schema.
  const tickets = new Map<
    string,
    {
      hash: string;
      ready: boolean;
      approved: boolean;
      mode: string | undefined;
      expires: number;
      promptPending: boolean;
      timer?: ReturnType<typeof setTimeout>;
      removeAbortListener?: () => void;
    }
  >();
  const cancelCall = (id: string) => {
    const ticket = tickets.get(id);
    if (!ticket) return;
    clearTimeout(ticket.timer);
    ticket.removeAbortListener?.();
    tickets.delete(id);
    changed();
  };
  const armExpiry = (id: string) => {
    const ticket = tickets.get(id);
    if (!ticket) return;
    clearTimeout(ticket.timer);
    ticket.timer = setTimeout(() => {
      if (tickets.get(id) === ticket && !ticket.promptPending) changed();
    }, 10 * 60_000);
    ticket.timer.unref();
  };
  const issue = (
    id: string,
    call: unknown,
    ready: boolean,
    mode: string | undefined,
  ) => {
    for (const [id, ticket] of tickets)
      if (!ticket.promptPending && ticket.expires < Date.now()) cancelCall(id);
    if (tickets.size >= 128) {
      const evictable = [...tickets].find(
        ([, ticket]) => !ticket.promptPending,
      );
      if (!evictable)
        throw new PluginPermissionError(
          "Too many plugin tool approvals are pending; finish one and try again.",
        );
      cancelCall(evictable[0]);
    }
    tickets.set(id, {
      hash: fingerprint(call),
      ready,
      approved: false,
      mode,
      expires: Date.now() + 10 * 60_000,
      promptPending: false,
    });
    armExpiry(id);
  };
  const ticketFor = (id: string, call: unknown) => {
    const ticket = tickets.get(id);
    if (!ticket || ticket.hash !== fingerprint(call))
      throw new PluginPermissionError(NO_CHECK);
    if (!ticket.promptPending && ticket.expires < Date.now())
      throw new PluginPermissionError(APPROVAL_EXPIRED);
    return ticket;
  };
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
  const rules = async () => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      const raw = await Promise.race([
        queryPermissionSettings(input.getQuery()),
        new Promise<never>((_, reject) => {
          timer = setTimeout(
            () => reject(new PluginPermissionError("getSettings timed out")),
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
      for (const behavior of ["allow", "deny", "ask"] as const)
        rules[behavior].push(...sessionRules[behavior]);
      return rules;
    } catch (error) {
      if (!logged) {
        logged = true;
        logger.warn(FAILURE, {
          threadId: input.threadId,
          err: error instanceof Error ? error.message : String(error),
        });
      }
      throw new PluginPermissionError(FAILURE);
    } finally {
      if (timer !== undefined) clearTimeout(timer);
    }
  };
  const evaluate = async (
    call: NonNullable<ReturnType<typeof target>>,
    mode = input.getMode(),
  ) => {
    const current = await rules();
    const matches = (behavior: Behavior) =>
      current[behavior].some((rule) =>
        matchesPluginRule(rule, call.name, call.args, behavior),
      );
    if (matches("deny"))
      return {
        decision: "deny" as const,
        reason: `Permission to use ${call.name} has been denied by a rule.`,
      };
    if (matches("ask"))
      return {
        decision: mode === "dontAsk" ? ("deny" as const) : ("ask" as const),
        reason: `Permission required for ${call.name}.`,
      };
    // Native plugin tools carry no read-only annotation, so Plan asks even
    // when an allow rule exists (observed with the real CLI).
    if (mode === "plan")
      return {
        decision: "ask" as const,
        reason: `Permission required for ${call.name} in plan mode.`,
      };
    if (mode === "bypassPermissions" || matches("allow"))
      return {
        decision: "allow" as const,
        reason: `Permission granted for ${call.name}.`,
      };
    return {
      decision: mode === "dontAsk" ? ("deny" as const) : ("ask" as const),
      reason: `Permission required for ${call.name}.`,
    };
  };
  const checkHook: HookCallback = async (event, _toolUseId, options) => {
    if (event.hook_event_name !== "PreToolUse" || event.tool_name !== BRIDGE)
      return {};
    const call = target(event.tool_input);
    const started = generation;
    let decision: Behavior = "deny";
    let reason =
      "Invalid plugin bridge target or arguments; list plugin tools again.";
    cancelCall(event.tool_use_id);
    try {
      if (call) {
        const mode = event.permission_mode ?? input.getMode();
        const outcome = await evaluate(call, mode);
        decision = outcome.decision;
        reason = outcome.reason;
        if (decision !== "deny") {
          if (
            !event.tool_use_id ||
            generation !== started ||
            options.signal.aborted
          )
            throw new PluginPermissionError(NO_CHECK);
          issue(event.tool_use_id, call, decision === "allow", mode);
        }
      }
    } catch (error) {
      decision = "deny";
      reason = error instanceof Error ? error.message : String(error);
    }
    return {
      hookSpecificOutput: {
        hookEventName: "PreToolUse",
        permissionDecision: decision,
        permissionDecisionReason: reason,
      },
    };
  };
  const checkCanUseTool: CanUseTool = async (name, args, options) => {
    if (name !== BRIDGE) {
      const started = generation;
      const result = await input.canUseTool(name, args, options);
      if (result.behavior === "allow" && started === generation)
        remember(result.updatedPermissions);
      return result;
    }
    const call = target(args);
    if (!call)
      return {
        behavior: "deny",
        message:
          "Invalid plugin bridge target or arguments; list plugin tools again.",
      };
    try {
      const outcome = await evaluate(
        call,
        tickets.get(options.toolUseID)?.mode,
      );
      if (outcome.decision === "deny") {
        cancelCall(options.toolUseID);
        return { behavior: "deny", message: outcome.reason };
      }
      const ticket = ticketFor(options.toolUseID, call);
      if (outcome.decision === "allow") {
        ticket.ready = true;
        return {
          behavior: "allow",
          updatedInput: { ...args, arguments: call.args },
        };
      }
      // The user may leave a prompt open indefinitely. Keep its ticket until
      // they answer; Stop/rebuild still invalidates it through clearTickets.
      ticket.promptPending = true;
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
      if (result.behavior !== "allow") {
        cancelCall(options.toolUseID);
        return result;
      }
      // Stop/rebuild invalidates an outstanding approval's old ticket too.
      if (tickets.get(options.toolUseID) !== ticket)
        throw new PluginPermissionError(APPROVAL_EXPIRED);
      const updated = {
        name: call.name,
        args: result.updatedInput ?? call.args,
      };
      ticket.hash = fingerprint(updated);
      ticket.ready = true;
      ticket.approved = true;
      ticket.promptPending = false;
      ticket.expires = Date.now() + 10 * 60_000;
      armExpiry(options.toolUseID);
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
        updatedInput: { ...args, arguments: updated.args },
        ...(updates === undefined ? {} : { updatedPermissions: updates }),
      };
    } catch (error) {
      cancelCall(options.toolUseID);
      return {
        behavior: "deny",
        message: error instanceof Error ? error.message : String(error),
      };
    }
  };
  const authorizeCall: PluginBridgeGuard["authorizeCall"] = async (
    args,
    extra,
  ) => {
    const call = target(args);
    const id =
      object(extra) && object(extra._meta)
        ? extra._meta["claudecode/toolUseId"]
        : undefined;
    if (!call || typeof id !== "string")
      throw new PluginPermissionError(NO_CHECK);
    const started = generation;
    const ticket = ticketFor(id, call);
    cancelCall(id); // A ticket is consumed once, even on a failed dispatch.
    if (!ticket.ready) throw new PluginPermissionError(NO_CHECK);
    const outcome = await evaluate(call, ticket.mode);
    if (generation !== started) throw new PluginPermissionError(NO_CHECK);
    if (
      outcome.decision === "deny" ||
      (outcome.decision === "ask" && !ticket.approved)
    )
      throw new PluginPermissionError(outcome.reason);
  };
  const filterList: PluginBridgeGuard["filterList"] = async (result) => {
    const current = await rules();
    if (
      !object(result) ||
      !object(result.structuredContent) ||
      !Array.isArray(result.structuredContent.tools)
    )
      throw new PluginPermissionError(
        "Codemux couldn't read the plugin tool catalog",
      );
    const tools = result.structuredContent.tools.filter(
      (tool) =>
        object(tool) &&
        typeof tool.name === "string" &&
        !current.deny.some((rule) =>
          matchesPluginRule(rule, `mcp__codemux__${tool.name}`, {}, "deny"),
        ),
    );
    const data = { ...result.structuredContent, tools };
    return {
      ...result,
      structuredContent: data,
      content: [{ type: "text", text: JSON.stringify(data) }],
    };
  };
  const clearTickets = () => {
    generation++;
    active = 0;
    for (const id of tickets.keys()) cancelCall(id);
  };
  // Session rules belong to one CLI process, just like native grants.
  const resetSessionRules = () => {
    sessionRules.allow = [];
    sessionRules.deny = [];
    sessionRules.ask = [];
    clearTickets();
  };
  const hook: HookCallback = async (...args) => {
    const done = beginCall();
    const [event, , options] = args;
    const id =
      event.hook_event_name === "PreToolUse" ? event.tool_use_id : undefined;
    const abort = () => {
      if (id) cancelCall(id);
    };
    if (id) options.signal.addEventListener("abort", abort, { once: true });
    try {
      return await checkHook(...args);
    } finally {
      const remove = () => options.signal.removeEventListener("abort", abort);
      const ticket = id ? tickets.get(id) : undefined;
      if (ticket) ticket.removeAbortListener = remove;
      else remove();
      done();
    }
  };
  const canUseTool: CanUseTool = async (...args) => {
    const done = beginCall();
    try {
      return await checkCanUseTool(...args);
    } finally {
      done();
    }
  };
  return {
    beginCall,
    isBusy: () =>
      active > 0 ||
      [...tickets.values()].some(
        (ticket) => ticket.promptPending || ticket.expires >= Date.now(),
      ),
    cancelCall,
    hook,
    canUseTool,
    authorizeCall,
    filterList,
    clearTickets,
    resetSessionRules,
  };
}
