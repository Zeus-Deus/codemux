export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

export interface Tool {
  name: string;
  description: string;
  inputSchema: Record<string, Json>;
}

export interface Initialize {
  provider: string;
  cwd: string;
  model: string | null;
  effort: string | null;
  tools: Tool[];
  options?: Record<string, Json>;
}

export interface Ready {
  protocolVersion: 1;
  provider: string;
  adapterVersion: string;
  sessionId: string;
  tools: string[];
  isolation: {
    nativeTools: string[];
    nativeFanout: false;
    ambientConfig: false;
  };
}

export interface Host {
  request(method: string, params: Json): Promise<Json>;
  notify(method: string, params: Json): void;
}

/** One process and one runtime belong to exactly one captured workflow attempt. */
export interface Runtime {
  initialize(input: Initialize): Promise<Ready>;
  startTurn(input: { turnId: string; prompt: string }): Promise<void>;
  cancel(): Promise<void>;
  dispose(): Promise<void>;
}

export function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function parseInitialize(value: unknown): Initialize {
  if (!object(value) || typeof value.provider !== "string" || typeof value.cwd !== "string"
      || value.cwd.length === 0 || !(value.model === null || typeof value.model === "string")
      || !(value.effort === null || typeof value.effort === "string")
      || !Array.isArray(value.tools) || value.tools.length === 0 || value.tools.length > 64) {
    throw new Error("Invalid managed workflow initialization");
  }
  const names = new Set<string>();
  const tools: Tool[] = value.tools.map((tool: unknown) => {
    if (!object(tool) || typeof tool.name !== "string" || !/^workflow_[a-z0-9_]+$/.test(tool.name)
        || names.has(tool.name) || typeof tool.description !== "string" || !object(tool.inputSchema)
        || tool.inputSchema.type !== "object" || JSON.stringify(tool.inputSchema).length > 64 * 1024) {
      throw new Error("Invalid or duplicate managed tool definition");
    }
    names.add(tool.name);
    // JSON-RPC parsed this structure from JSON; there are no callbacks or prototypes.
    return { name: tool.name, description: tool.description, inputSchema: tool.inputSchema as Record<string, Json> };
  });
  if (value.options !== undefined && !object(value.options)) throw new Error("Invalid provider options");
  return {
    provider: value.provider, cwd: value.cwd, model: value.model, effort: value.effort, tools,
    ...(value.options === undefined ? {} : { options: value.options as Record<string, Json> }),
  };
}
