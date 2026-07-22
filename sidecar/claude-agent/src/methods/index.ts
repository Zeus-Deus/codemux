// Method registry. `main.ts` imports a single `buildMethods(emit)`
// function and dispatches incoming JSON-RPC calls against it.
//
// Each method is a plain async function from `unknown` params to an
// `unknown` result. Validation lives inside each handler — failures
// throw a regular `Error`, which `main.ts` turns into a structured
// JSON-RPC error response.

import type { PermissionMode } from "@anthropic-ai/claude-agent-sdk";

import {
  probeAuthenticated,
  probeInstalled,
} from "../auth-probe.ts";
import type { EventEmitter } from "../session.ts";
import {
  ClaudeSession,
  type SendTurnInput,
  type SessionStartInput,
} from "../session.ts";
import type { ApprovalDecision } from "../permissions.ts";
import { listCommands } from "./list-commands.ts";
import { listModels } from "./list-models.ts";
import { ping } from "./ping.ts";

/** The live sessions this process is managing, keyed by `threadId`. */
const sessions = new Map<string, ClaudeSession>();

/** Signature every method handler must satisfy. */
export type MethodHandler = (params: unknown) => Promise<unknown>;

/** Human-visible error thrown by handlers that want JSON-RPC
 *  -32602 invalid-params semantics. Main.ts surfaces this code. */
export class InvalidParamsError extends Error {
  override readonly name = "InvalidParamsError";
}

// ---------------------------------------------------------------------------
// Param validation helpers
// ---------------------------------------------------------------------------

function asObject(p: unknown, what: string): Record<string, unknown> {
  if (p === null || p === undefined || typeof p !== "object" || Array.isArray(p)) {
    throw new InvalidParamsError(`${what} requires an object payload`);
  }
  return p as Record<string, unknown>;
}

function asString(v: unknown, field: string): string {
  if (typeof v !== "string") {
    throw new InvalidParamsError(`${field} must be a string`);
  }
  return v;
}

function optString(v: unknown, field: string): string | undefined {
  if (v === undefined || v === null) return undefined;
  if (typeof v !== "string") {
    throw new InvalidParamsError(`${field} must be a string when present`);
  }
  return v;
}

function optBoolean(v: unknown, field: string): boolean | undefined {
  if (v === undefined || v === null) return undefined;
  if (typeof v !== "boolean") {
    throw new InvalidParamsError(`${field} must be a boolean when present`);
  }
  return v;
}

function optArray(v: unknown, field: string): unknown[] | undefined {
  if (v === undefined || v === null) return undefined;
  if (!Array.isArray(v)) {
    throw new InvalidParamsError(`${field} must be an array when present`);
  }
  return v;
}

function optRecord(
  v: unknown,
  field: string,
): Record<string, unknown> | undefined {
  if (v === undefined || v === null) return undefined;
  if (typeof v !== "object" || Array.isArray(v)) {
    throw new InvalidParamsError(`${field} must be an object when present`);
  }
  return v as Record<string, unknown>;
}

function getSession(threadId: string): ClaudeSession {
  const s = sessions.get(threadId);
  if (!s) {
    throw new InvalidParamsError(`no session for thread ${threadId}`);
  }
  return s;
}

// ---------------------------------------------------------------------------
// Method handlers
// ---------------------------------------------------------------------------

function makeStartSession(emit: EventEmitter): MethodHandler {
  return async (params) => {
    const p = asObject(params, "start-session");
    const threadId = asString(p["threadId"], "threadId");
    if (sessions.has(threadId)) {
      throw new InvalidParamsError(
        `session already exists for thread ${threadId}`,
      );
    }
    const cwd = asString(p["cwd"], "cwd");
    const pathToClaudeCodeExecutable = asString(
      p["pathToClaudeCodeExecutable"],
      "pathToClaudeCodeExecutable",
    );
    const input: SessionStartInput = {
      threadId,
      cwd,
      pathToClaudeCodeExecutable,
    };
    const model = optString(p["model"], "model");
    if (model !== undefined) input.model = model;
    const effort = optString(p["effort"], "effort");
    if (effort !== undefined) input.effort = effort;
    const fastMode = optBoolean(p["fastMode"], "fastMode");
    if (fastMode !== undefined) input.fastMode = fastMode;
    const permissionMode = optString(p["permissionMode"], "permissionMode");
    if (permissionMode !== undefined) {
      input.permissionMode = permissionMode as PermissionMode;
    }
    if (p["allowDangerouslySkipPermissions"] !== undefined) {
      input.allowDangerouslySkipPermissions = Boolean(
        p["allowDangerouslySkipPermissions"],
      );
    }
    const addl = optArray(p["additionalDirectories"], "additionalDirectories");
    if (addl !== undefined) {
      input.additionalDirectories = addl.map((x, i) =>
        asString(x, `additionalDirectories[${i}]`),
      );
    }
    const settings = optRecord(p["settings"], "settings");
    if (settings !== undefined) input.settings = settings;
    const resume = optString(p["resume"], "resume");
    if (resume !== undefined) input.resume = resume;
    const sessionId = optString(p["sessionId"], "sessionId");
    if (sessionId !== undefined) input.sessionId = sessionId;
    const extraArgs = optRecord(p["extraArgs"], "extraArgs");
    if (extraArgs !== undefined) {
      input.extraArgs = Object.fromEntries(
        Object.entries(extraArgs).map(([k, v]) => {
          if (v === null || v === undefined) return [k, null] as const;
          if (typeof v !== "string") {
            throw new InvalidParamsError(
              `extraArgs.${k} must be a string or null`,
            );
          }
          return [k, v] as const;
        }),
      );
    }
    // Stage 3 — MCP tools registered with the SDK as the in-process
    // `codemux` virtual MCP server. Validation is structural only;
    // each entry's prefixedName is the agent-facing identifier and
    // the inputSchema is JSON Schema.
    const rawMcpTools = optArray(p["mcpTools"], "mcpTools");
    if (rawMcpTools && rawMcpTools.length > 0) {
      input.mcpTools = rawMcpTools.map((raw, idx) => {
        const obj = asObject(raw, `mcpTools[${idx}]`);
        return {
          name: asString(obj["name"], `mcpTools[${idx}].name`),
          prefixedName: asString(
            obj["prefixedName"],
            `mcpTools[${idx}].prefixedName`,
          ),
          description:
            obj["description"] === null || obj["description"] === undefined
              ? null
              : asString(obj["description"], `mcpTools[${idx}].description`),
          inputSchema: obj["inputSchema"] ?? {},
          serverId: asString(obj["serverId"], `mcpTools[${idx}].serverId`),
        };
      });
    }

    const session = new ClaudeSession(input, emit);
    sessions.set(threadId, session);
    return {
      threadId,
      pathToClaudeCodeExecutable,
    };
  };
}

const sendTurn: MethodHandler = async (params) => {
  const p = asObject(params, "send-turn");
  const threadId = asString(p["threadId"], "threadId");
  const session = getSession(threadId);
  const text = asString(p["text"], "text");
  const input: SendTurnInput = { text };
  const modelOverride = optString(p["modelOverride"], "modelOverride");
  if (modelOverride !== undefined) input.modelOverride = modelOverride;
  // Stage 6 — image attachments. Bytes arrive base64-encoded so the
  // JSON-RPC frame stays text-only; we don't decode them here, the
  // SDK accepts the same shape verbatim as part of its multimodal
  // user message content array.
  const rawImages = optArray(p["images"], "images");
  if (rawImages && rawImages.length > 0) {
    input.images = rawImages.map((raw, idx) => {
      const obj = asObject(raw, `images[${idx}]`);
      return {
        mediaType: asString(obj["mediaType"], `images[${idx}].mediaType`),
        dataBase64: asString(obj["dataBase64"], `images[${idx}].dataBase64`),
      };
    });
  }
  await session.sendTurn(input);
  return { turnStarted: true };
};

const interrupt: MethodHandler = async (params) => {
  const p = asObject(params, "interrupt");
  const threadId = asString(p["threadId"], "threadId");
  const session = getSession(threadId);
  await session.interrupt();
  return {};
};

const setModel: MethodHandler = async (params) => {
  const p = asObject(params, "set-model");
  const threadId = asString(p["threadId"], "threadId");
  const session = getSession(threadId);
  const raw = p["model"];
  const model = raw === null || raw === undefined ? undefined : asString(raw, "model");
  await session.setModel(model);
  return {};
};

const updateMcpTools: MethodHandler = async (params) => {
  const p = asObject(params, "update-mcp-tools");
  const threadId = asString(p["threadId"], "threadId");
  const session = getSession(threadId);
  const rawTools = optArray(p["mcpTools"], "mcpTools") ?? [];
  const tools = rawTools.map((raw, idx) => {
    const obj = asObject(raw, `mcpTools[${idx}]`);
    return {
      name: asString(obj["name"], `mcpTools[${idx}].name`),
      prefixedName: asString(
        obj["prefixedName"],
        `mcpTools[${idx}].prefixedName`,
      ),
      description:
        obj["description"] === null || obj["description"] === undefined
          ? null
          : asString(obj["description"], `mcpTools[${idx}].description`),
      inputSchema: obj["inputSchema"] ?? {},
      serverId: asString(obj["serverId"], `mcpTools[${idx}].serverId`),
    };
  });
  await session.updateMcpTools(tools);
  return { ok: true, count: tools.length };
};

const setPermissionMode: MethodHandler = async (params) => {
  const p = asObject(params, "set-permission-mode");
  const threadId = asString(p["threadId"], "threadId");
  const session = getSession(threadId);
  const mode = asString(p["mode"], "mode") as PermissionMode;
  await session.setPermissionMode(mode);
  return {};
};

const respondToRequest: MethodHandler = async (params) => {
  const p = asObject(params, "respond-to-request");
  const threadId = asString(p["threadId"], "threadId");
  const session = getSession(threadId);
  const requestId = asString(p["requestId"], "requestId");
  const decisionRaw = asObject(p["decision"], "decision");
  const behavior = asString(decisionRaw["behavior"], "decision.behavior");
  let decision: ApprovalDecision;
  if (behavior === "allow") {
    decision = {
      behavior: "allow",
      updatedInput: decisionRaw["updatedInput"],
    };
    if (decisionRaw["updatedPermissions"] !== undefined) {
      decision.updatedPermissions = decisionRaw["updatedPermissions"];
    }
  } else if (behavior === "deny") {
    decision = {
      behavior: "deny",
      message: asString(decisionRaw["message"], "decision.message"),
    };
    if (decisionRaw["interrupt"] !== undefined) {
      decision.interrupt = Boolean(decisionRaw["interrupt"]);
    }
  } else {
    throw new InvalidParamsError(
      `decision.behavior must be "allow" or "deny", got ${behavior}`,
    );
  }
  await session.respondToRequest(requestId, decision);
  return {};
};

const respondToUserInput: MethodHandler = async (params) => {
  const p = asObject(params, "respond-to-user-input");
  const threadId = asString(p["threadId"], "threadId");
  const session = getSession(threadId);
  const requestId = asString(p["requestId"], "requestId");
  const answers = optArray(p["answers"], "answers") ?? [];
  await session.respondToUserInput(requestId, answers);
  return {};
};

const initializationResult: MethodHandler = async (params) => {
  const p = asObject(params, "initialization-result");
  const threadId = asString(p["threadId"], "threadId");
  const session = getSession(threadId);
  return session.initializationResult();
};

const stopSession: MethodHandler = async (params) => {
  const p = asObject(params, "stop-session");
  const threadId = asString(p["threadId"], "threadId");
  const session = sessions.get(threadId);
  if (!session) {
    return { alreadyClosed: true };
  }
  sessions.delete(threadId);
  await session.close();
  return {};
};

const probeInstalledMethod: MethodHandler = async (params) => {
  const p = params === undefined || params === null ? {} : asObject(params, "probe-installed");
  const binaryPath = optString(p["binaryPath"], "binaryPath") ?? "claude";
  return probeInstalled(binaryPath);
};

const probeAuthenticatedMethod: MethodHandler = async (params) => {
  const p = params === undefined || params === null ? {} : asObject(params, "probe-authenticated");
  const binaryPath = optString(p["binaryPath"], "binaryPath") ?? "claude";
  return probeAuthenticated(binaryPath);
};

const listModelsMethod: MethodHandler = async (params) => {
  const p = asObject(params, "list-models");
  return listModels({
    cwd: asString(p["cwd"], "cwd"),
    pathToClaudeCodeExecutable: asString(
      p["pathToClaudeCodeExecutable"],
      "pathToClaudeCodeExecutable",
    ),
  });
};

const listCommandsMethod: MethodHandler = async (params) => {
  const p = asObject(params, "list-commands");
  return listCommands({
    cwd: asString(p["cwd"], "cwd"),
    pathToClaudeCodeExecutable: asString(
      p["pathToClaudeCodeExecutable"],
      "pathToClaudeCodeExecutable",
    ),
  });
};

// ---------------------------------------------------------------------------
// Public: build the registry
// ---------------------------------------------------------------------------

/** Construct the full method registry. `emit` is the event emitter
 *  that forwards notifications to stdout — sessions capture it on
 *  creation and use it for their message stream. */
export function buildMethods(emit: EventEmitter): Record<string, MethodHandler> {
  return {
    ping: (params) => ping((params ?? {}) as Record<string, unknown>),
    "start-session": makeStartSession(emit),
    "send-turn": sendTurn,
    interrupt,
    "set-model": setModel,
    "set-permission-mode": setPermissionMode,
    "update-mcp-tools": updateMcpTools,
    "respond-to-request": respondToRequest,
    "respond-to-user-input": respondToUserInput,
    "initialization-result": initializationResult,
    "stop-session": stopSession,
    "probe-installed": probeInstalledMethod,
    "probe-authenticated": probeAuthenticatedMethod,
    "list-models": listModelsMethod,
    "list-commands": listCommandsMethod,
  };
}

/** Test-only: drop every live session without going through the RPC
 *  layer. Used between test cases to avoid state leaking. */
export function _resetSessionsForTests(): void {
  sessions.clear();
}

/** Test-only: inspect the internal map. */
export function _peekSessionsForTests(): Map<string, ClaudeSession> {
  return sessions;
}
