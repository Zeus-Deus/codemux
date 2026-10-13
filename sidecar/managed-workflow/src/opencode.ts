import { mkdir, access, chmod } from "node:fs/promises";
import { isAbsolute, join, resolve } from "node:path";
import { randomBytes, timingSafeEqual } from "node:crypto";
import type { Host, Initialize, Json, Ready, Runtime, Tool } from "./contract";
import { object } from "./contract";

export const OPENCODE_ADAPTER_VERSION = "opencode-managed-v1";
// The isolation controls below were reviewed against this exact native source.
// A new version must pass the native readiness test before extending this pin.
export const OPENCODE_NATIVE_VERSION = "1.18.35";
export const OPENCODE_NATIVE_VERSIONS = [OPENCODE_NATIVE_VERSION, "2.0.27"] as const;
function reviewedVersion(version: string): string {
  if (!OPENCODE_NATIVE_VERSIONS.some((pin) => pin === version)) throw new Error(`Managed OpenCode requires a reviewed native version (${OPENCODE_NATIVE_VERSIONS.join(", ")})`);
  return version;
}
// Remove native tool/ambient discovery plugins, not policy, provider, config-agent
// or private MCP plumbing. These are exact v2.0.27 built-in IDs.
const V2_DISABLED_PLUGINS = [
  "opencode.tool.patch", "opencode.tool.edit", "opencode.tool.glob", "opencode.tool.grep",
  "opencode.tools", "opencode.tools.mcp-resources", "opencode.tool.question", "opencode.tool.read",
  "opencode.tool.shell", "opencode.tool.skill", "opencode.tool.subagent", "opencode.tool.webfetch",
  "opencode.tool.websearch", "opencode.tool.write", "opencode.agent", "opencode.plan", "opencode.skill",
  "opencode.command", "opencode.vcs.git", "opencode.vcs.hg", "opencode.warming",
  "opencode.config.compatibility", "opencode.config.skill", "opencode.config.instruction", "opencode.config.reference",
  "opencode.browser",
].map((id) => `-${id}`);
const AGENT = "codemux-workflow";
const MAX_BODY = 2 * 1024 * 1024;
const MAX_RESPONSE = 8 * 1024 * 1024;

export function permissions(tools: Tool[]): Record<string, string> {
  return Object.fromEntries([
    ["*", "deny"], ["external_directory", "deny"],
    ...tools.map((tool) => [`codemux_${tool.name}`, "allow"]),
  ]);
}

function sameJson(left: unknown, right: unknown): boolean {
  const canonical = (value: unknown): unknown => Array.isArray(value) ? value.map(canonical)
    : object(value) ? Object.fromEntries(Object.keys(value).sort().map((key) => [key, canonical(value[key])])) : value;
  return JSON.stringify(canonical(left)) === JSON.stringify(canonical(right));
}

export function sessionPermissions(tools: Tool[], version = OPENCODE_NATIVE_VERSION): Json[] {
  return Object.entries(permissions(tools)).map(([permission, action]) => version === "2.0.27"
    ? { action: permission, resource: "*", effect: action } : { permission, pattern: "*", action });
}

function modelRef(input: Initialize): Record<string, Json> {
  const slash = input.model!.indexOf("/");
  return { providerID: input.model!.slice(0, slash), id: input.model!.slice(slash + 1), ...(input.effort ? { variant: input.effort } : {}) };
}

export function managedConfig(input: Initialize, url: string, token: string, version = OPENCODE_NATIVE_VERSION): Record<string, Json> {
  reviewedVersion(version);
  const rule = permissions(input.tools);
  if (version === "2.0.27") {
    const rules = sessionPermissions(input.tools, version);
    return {
      model: { providerID: modelRef(input).providerID!, model: modelRef(input).id!, ...(input.effort ? { variant: input.effort } : {}) }, default_agent: AGENT, permissions: rules,
      plugins: V2_DISABLED_PLUGINS, skills: [], instructions: [],
      lsp: false, formatter: false, update: "disable", snapshots: false, share: "disabled",
      warming: false, compaction: { auto: false },
      agents: { [AGENT]: { mode: "primary", permissions: rules, steps: 128 } },
      experimental: { policies: [
        { action: "provider.use", resource: "*", effect: "deny" },
        { action: "provider.use", resource: modelRef(input).providerID!, effect: "allow" },
        { action: "integration.use", resource: "*", effect: "deny" },
        { action: "integration.use", resource: "mcp:codemux", effect: "allow" },
      ] },
      mcp: { servers: { codemux: { type: "remote", url, oauth: false, disabled: false, codemode: false, headers: { Authorization: `Bearer ${token}` } } } },
    };
  }
  return {
    model: input.model!, enabled_providers: [input.model!.slice(0, input.model!.indexOf("/"))],
    default_agent: AGENT, permission: rule, plugin: [], instructions: [],
    lsp: false, formatter: false, autoupdate: false, snapshot: false, share: "disabled",
    compaction: { auto: false, prune: false },
    agent: { [AGENT]: { mode: "primary", permission: rule, steps: 128 } },
    mcp: { codemux: { type: "remote", url, oauth: false, enabled: true, headers: { Authorization: `Bearer ${token}` } } },
  };
}

/** Check the configuration of the process that will actually receive the prompt. */
export function assertRuntime(config: unknown, agents: unknown, expected: Record<string, Json>, tools: Tool[]): void {
  if (!object(config) || !sameJson(config.permission, expected.permission)
      || !sameJson(config.mcp, expected.mcp)
      || config.default_agent !== AGENT || config.model !== expected.model
      || !Array.isArray(config.plugin) || config.plugin.length !== 0
      || !Array.isArray(config.instructions) || config.instructions.length !== 0
      || config.lsp !== false || config.formatter !== false || config.autoupdate !== false
      || config.snapshot !== false || config.share !== "disabled" || !object(config.compaction)
      || config.compaction.auto !== false || config.compaction.prune !== false || !Array.isArray(agents)) {
    throw new Error("OpenCode did not confirm the isolated managed configuration");
  }
  const agent: unknown = agents.find((entry: unknown) => object(entry) && entry.name === AGENT);
  if (!object(agent) || agent.mode !== "primary" || !Array.isArray(agent.permission)) {
    throw new Error("OpenCode managed agent is missing");
  }
  // OpenCode adds a tool-output directory exception to the agent. The session's
  // verified final rules (below) override this and every native default.
  const wanted = sessionPermissions(tools);
  const actual = agent.permission;
  if (!actual.some((_: unknown, index: number) => sameJson(actual.slice(index, index + wanted.length), wanted))) {
    throw new Error("OpenCode resolved agent permissions differ from the managed allowlist");
  }
}

/** Private MCP endpoint; caller identity never comes from tool arguments. */
export function privateMcp(host: Host, tools: Tool[]) {
  const token = randomBytes(32).toString("hex");
  const expected = Buffer.from(`Bearer ${token}`);
  const server = Bun.serve({ hostname: "127.0.0.1", port: 0, maxRequestBodySize: MAX_BODY,
    async fetch(request) {
      const auth = Buffer.from(request.headers.get("authorization") ?? "");
      if (auth.length !== expected.length || !timingSafeEqual(auth, expected)) return new Response(null, { status: 401 });
      if (new URL(request.url).pathname !== "/mcp") return new Response(null, { status: 404 });
      if (request.method !== "POST") return new Response(null, { status: 405 });
      if (Number(request.headers.get("content-length") ?? "0") > MAX_BODY) return new Response(null, { status: 413 });
      const text = await request.text();
      if (Buffer.byteLength(text) > MAX_BODY) return new Response(null, { status: 413 });
      let value: unknown;
      try { value = JSON.parse(text); } catch { return new Response(null, { status: 400 }); }
      if (!object(value) || value.jsonrpc !== "2.0" || typeof value.method !== "string") return new Response(null, { status: 400 });
      if (value.id === undefined && value.method === "notifications/initialized") return new Response(null, { status: 202 });
      if (!(typeof value.id === "string" || typeof value.id === "number")) return new Response(null, { status: 400 });
      const response = (result: unknown) => Response.json({ jsonrpc: "2.0", id: value.id, result });
      switch (value.method) {
        case "initialize": return response({ protocolVersion: "2024-11-05", capabilities: { tools: {} }, serverInfo: { name: "codemux-managed", version: "1" } });
        case "ping": return response({});
        case "tools/list": return response({ tools });
        case "tools/call": {
          const params = value.params;
          if (!object(params) || typeof params.name !== "string"
              || !tools.some((tool) => tool.name === params.name) || !object(params.arguments)) {
            return Response.json({ jsonrpc: "2.0", id: value.id, error: { code: -32602, message: "Managed tool is unavailable" } });
          }
          try {
            const result = await host.request("managed/tool_call", { name: params.name, arguments: params.arguments as Json });
            return response({ content: [{ type: "text", text: JSON.stringify(result) }] });
          } catch (error) {
            // Host validation errors explain repairable graph/scope failures.
            const message = error instanceof Error ? error.message.slice(0, 2000) : "Managed host tool rejected this call";
            return response({ isError: true, content: [{ type: "text", text: message }] });
          }
        }
        default: return Response.json({ jsonrpc: "2.0", id: value.id, error: { code: -32601, message: "Method is unavailable" } });
      }
    },
  });
  return { url: `http://127.0.0.1:${server.port}/mcp`, token, stop: () => server.stop(true) };
}

export interface NativeServer { url: string; password: string; stop(): Promise<void>; dead(): boolean }
export interface OpenCodeDeps {
  version?(binary: string, cwd: string, env: NodeJS.ProcessEnv): Promise<string>;
  launch(binary: string, cwd: string, env: NodeJS.ProcessEnv): Promise<NativeServer>;
}

export function nativeTokens(value: unknown): Record<string, number> | undefined {
  if (!object(value) || !object(value.cache)) return undefined;
  const numbers = [value.input, value.output, value.reasoning, value.cache.read, value.cache.write];
  if (!numbers.every((number): number is number => typeof number === "number" && Number.isSafeInteger(number) && number >= 0)) return undefined;
  const [input = 0, output = 0, reasoning = 0, read = 0, write = 0] = numbers;
  if (!Number.isSafeInteger(input + output + reasoning + read + write)) return undefined;
  // OpenCode's output and reasoning are disjoint. Canonical output includes reasoning.
  return { inputTokens: input, outputTokens: output + reasoning, reasoningTokens: reasoning, cacheReadTokens: read, cacheWriteTokens: write };
}

function cleanEnvironment(stateDir: string, config: Record<string, Json>): NodeJS.ProcessEnv {
  const env = { ...process.env };
  for (const name of Object.keys(env)) {
    if (name.startsWith("OPENCODE_") || name.startsWith("OTEL_") || ["NODE_OPTIONS", "BUN_OPTIONS", "BUN_INSPECT", "BUN_PRELOAD", "LD_PRELOAD", "LD_LIBRARY_PATH", "DYLD_INSERT_LIBRARIES"].includes(name)) delete env[name];
  }
  return { ...env, XDG_CONFIG_HOME: join(stateDir, "config"), XDG_DATA_HOME: join(stateDir, "data"),
    XDG_CACHE_HOME: join(stateDir, "cache"), XDG_STATE_HOME: join(stateDir, "state"),
    HOME: join(stateDir, "home"), USERPROFILE: join(stateDir, "home"),
    TMPDIR: join(stateDir, "tmp"), TMP: join(stateDir, "tmp"), TEMP: join(stateDir, "tmp"), OPENCODE_TEST_HOME: join(stateDir, "home"),
    OPENCODE_CONFIG_CONTENT: JSON.stringify(config), OPENCODE_PURE: "true", OPENCODE_DISABLE_DEFAULT_PLUGINS: "true",
    OPENCODE_DISABLE_PROJECT_CONFIG: "true", OPENCODE_DISABLE_CLAUDE_CODE: "true", OPENCODE_DISABLE_EXTERNAL_SKILLS: "true",
    OPENCODE_DISABLE_AUTOUPDATE: "true", OPENCODE_DISABLE_MODELS_FETCH: "true", OPENCODE_DISABLE_LSP_DOWNLOAD: "true",
    OPENCODE_EXPERIMENTAL_DISABLE_FILEWATCHER: "true", OPENCODE_DISABLE_FILEWATCHER: "true", OPENCODE_FILEWATCHER_DISABLE: "true", OPENCODE_DISABLE_AUTOCOMPACT: "true", OPENCODE_DISABLE_PRUNE: "true",
  };
}

export const nativeDeps: OpenCodeDeps = {
  async version(binary, cwd, env) {
    const probe = Bun.spawn([binary, "--version"], { cwd, env, stdout: "pipe", stderr: "ignore" });
    const timer = setTimeout(() => probe.kill("SIGKILL"), 10_000);
    try {
      const version = await new Response(probe.stdout).text();
      if (await probe.exited !== 0) throw new Error("Managed OpenCode version probe failed");
      return reviewedVersion(version.trim() === "opencode v2.0.27" ? "2.0.27" : version.trim());
    } finally { clearTimeout(timer); probe.kill("SIGKILL"); await probe.exited; }
  },
  async launch(binary, cwd, env) {
    const password = randomBytes(32).toString("hex");
    const child = Bun.spawn([binary, "serve", "--hostname=127.0.0.1", "--port=0"], {
      cwd, env: { ...env, OPENCODE_SERVER_PASSWORD: password, OPENCODE_SERVER_USERNAME: "opencode" }, stdout: "pipe", stderr: "ignore",
    });
    try {
      const url = await new Promise<string>((resolve, reject) => {
        const timeout = setTimeout(() => reject(new Error("Managed OpenCode readiness timed out")), 20_000);
        void child.exited.then(() => { clearTimeout(timeout); reject(new Error("Managed OpenCode exited before readiness")); });
        void (async () => {
          let buffer = ""; let ready = false;
          for await (const bytes of child.stdout) {
            if (ready) continue;
            buffer += new TextDecoder().decode(bytes);
            if (buffer.length > 64 * 1024) throw new Error("OpenCode readiness output exceeded its limit");
            const match = buffer.match(/(?:opencode )?server listening on (http:\/\/127\.0\.0\.1:([0-9]+))/);
            if (match && Number(match[2]) > 0 && Number(match[2]) <= 65535) {
              clearTimeout(timeout); resolve(match[1]!);
              // Keep draining the owned child's stdout without exposing native logs.
              ready = true; buffer = "";
            }
          }
        })().catch((error: unknown) => { clearTimeout(timeout); reject(error); });
      });
      return { url, password, dead: () => child.exitCode !== null || child.signalCode !== null,
        async stop() { child.kill("SIGKILL"); await child.exited; } };
    } catch (error) { child.kill("SIGKILL"); await child.exited; throw error; }
  },
};

export class OpenCodeRuntime implements Runtime {
  private native?: NativeServer;
  private mcp?: ReturnType<typeof privateMcp>;
  private input?: Initialize;
  private session?: string;
  private selectedModel?: Record<string, Json>;
  private active: { id: string; abort: AbortController; cancelled: boolean; admission?: Promise<void>; cancellation?: Promise<void> } | undefined;
  private disposed = false;
  private version: string = OPENCODE_NATIVE_VERSION;
  private knownMessages = new Set<string>();
  constructor(private readonly host: Host, private readonly deps: OpenCodeDeps = nativeDeps) {}

  private async request(path: string, body?: Json, signal?: AbortSignal, method = body === undefined ? "GET" : "POST", noContent = false): Promise<unknown> {
    if (!this.native || this.native.dead()) throw new Error("Managed OpenCode server is unavailable");
    const response = await fetch(`${this.native.url}${path}`, {
      method, redirect: "error",
      headers: { Authorization: `Basic ${Buffer.from(`opencode:${this.native.password}`).toString("base64")}`, "Content-Type": "application/json" },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      signal: signal ?? AbortSignal.timeout(15_000),
    });
    if (!response.ok) throw new Error(`Managed OpenCode HTTP request failed (${response.status})`);
    if (noContent) {
      if (response.status !== 204) throw new Error("OpenCode did not confirm native quiescence");
      return undefined;
    }
    if (response.status === 204) throw new Error("Managed OpenCode response is empty");
    const reader = response.body?.getReader();
    if (!reader) throw new Error("Managed OpenCode response is empty");
    let text = ""; let bytes = 0; const decoder = new TextDecoder();
    try { while (true) { const item = await reader.read(); if (item.done) break;
      bytes += item.value.byteLength;
      if (bytes > MAX_RESPONSE) throw new Error("Managed OpenCode response exceeded its limit");
      text += decoder.decode(item.value, { stream: true });
    } } finally { await reader.cancel(); }
    return JSON.parse(text + decoder.decode());
  }

  async initialize(input: Initialize): Promise<Ready> {
    if (this.input || input.provider !== "opencode" || !input.model || !/^[a-zA-Z0-9._-]+\/.+$/.test(input.model)
        || typeof input.options?.nativeBinary !== "string" || !isAbsolute(input.options.nativeBinary)
        || typeof input.options.stateDir !== "string" || !isAbsolute(input.options.stateDir)) {
      throw new Error("Managed OpenCode requires an explicit provider/model and owned native runtime");
    }
    this.input = input;
    const stateDir = input.options.stateDir;
    // Managed administrator configuration is authoritative; never bypass it.
    try { await access("/etc/opencode"); throw new Error("Managed OpenCode cannot isolate administrator configuration"); }
    catch (error) { if (!object(error) || error.code !== "ENOENT") throw error; }
    for (const dir of ["home", "config", "data", "cache", "state", "tmp"]) await mkdir(join(stateDir, dir), { recursive: true, mode: 0o700 });
    // The native loader skips dependency installation for nonwritable config
    // directories. No plugins/tools need packages in this deliberately empty dir.
    const configDir = join(stateDir, "config", "opencode");
    await mkdir(configDir, { mode: 0o700 });
    if (process.platform === "linux") await chmod(configDir, 0o500);
    this.mcp = privateMcp(this.host, input.tools);
    try {
      this.version = reviewedVersion(await this.deps.version?.(input.options.nativeBinary, input.cwd, cleanEnvironment(stateDir, {})) ?? OPENCODE_NATIVE_VERSION);
      const expected = managedConfig(input, this.mcp.url, this.mcp.token, this.version);
      this.native = await this.deps.launch(input.options.nativeBinary, input.cwd, cleanEnvironment(stateDir, expected));
      if (this.version === "2.0.27") return await this.initializeV2(input, expected);
      const health = await this.request("/global/health");
      if (!object(health) || health.healthy !== true || health.version !== OPENCODE_NATIVE_VERSION) throw new Error("Managed OpenCode runtime version was not confirmed");
      assertRuntime(await this.request("/config"), await this.request("/agent"), expected, input.tools);
      const mcp = await this.request("/mcp");
      if (!object(mcp) || Object.keys(mcp).length !== 1 || !object(mcp.codemux) || mcp.codemux.status !== "connected") throw new Error("OpenCode did not connect to the private managed MCP server");
      const providers = await this.request("/provider");
      const slash = input.model.indexOf("/"); const providerID = input.model.slice(0, slash); const modelID = input.model.slice(slash + 1);
      if (!object(providers) || !Array.isArray(providers.connected) || !providers.connected.includes(providerID) || !Array.isArray(providers.all)) {
        throw new Error("Managed OpenCode requires a built-in provider authenticated through an environment API key; native OAuth and custom plugin routes are unavailable");
      }
      const provider: unknown = providers.all.find((entry: unknown) => object(entry) && entry.id === providerID);
      const model = object(provider) && object(provider.models) ? provider.models[modelID] : undefined;
      if (!object(model) || (input.effort && (!object(model.variants) || !object(model.variants[input.effort])))) throw new Error("Managed OpenCode model or effort variant is unavailable");
      const rules = sessionPermissions(input.tools);
      const session = await this.request("/session", { title: "CodeMux managed workflow", permission: rules });
      if (!object(session) || typeof session.id !== "string" || !/^ses_[a-zA-Z0-9]+$/.test(session.id)
          || !sameJson(session.permission, rules)) throw new Error("OpenCode did not confirm exact final session permissions");
      this.session = session.id;
      return { protocolVersion: 1, provider: "opencode", adapterVersion: OPENCODE_ADAPTER_VERSION, sessionId: session.id,
        tools: input.tools.map((tool) => tool.name), isolation: { nativeTools: [], nativeFanout: false, ambientConfig: false } };
    } catch (error) { await this.dispose(); throw error; }
  }

  private locationData(value: unknown): unknown[] {
    if (!object(value) || !sameJson(value.location, { directory: this.input!.cwd }) || !Array.isArray(value.data)) throw new Error("OpenCode location-scoped catalog was malformed");
    return value.data;
  }

  private assertSessionV2(value: unknown, expectedID?: string): Record<string, unknown> {
    if (!object(value) || typeof value.id !== "string" || !/^ses_[a-zA-Z0-9]+$/.test(value.id)
        || (expectedID && value.id !== expectedID) || value.parentID !== undefined
        || !sameJson(value.location, { directory: this.input!.cwd }) || value.agent !== AGENT
        || !object(value.model) || !sameJson({ ...value.model, variant: value.model.variant ?? "default" }, { ...this.selectedModel, variant: this.selectedModel?.variant ?? "default" })
        || !sameJson(value.permissions, sessionPermissions(this.input!.tools, this.version))) throw new Error("OpenCode did not confirm exact final session permissions, model, agent and location");
    return value;
  }

  private async initializeV2(input: Initialize, expected: Record<string, Json>): Promise<Ready> {
    const info = await this.request("/api/info");
    if (!object(info) || info.version !== this.version || typeof info.pid !== "number" || !Number.isSafeInteger(info.pid) || info.pid <= 0) throw new Error("Managed OpenCode runtime version was not confirmed");
    const query = `?location[directory]=${encodeURIComponent(input.cwd)}`;
    const entries = await this.request(`/api/config${query}`);
    const roots = [join(String(input.options!.stateDir), "config", "opencode"), join(String(input.options!.stateDir), "home", ".opencode")];
    if (!Array.isArray(entries) || !entries.some((entry) => object(entry) && entry.type === "document" && sameJson(entry.info, expected))
        || entries.some((entry) => !object(entry) || (entry.type !== "directory" && entry.type !== "document")
          || (entry.path !== undefined && (typeof entry.path !== "string" || !isAbsolute(entry.path) || resolve(entry.path) !== entry.path || !roots.some((root) => entry.path === root || (entry.type === "document" && typeof entry.path === "string" && entry.path.startsWith(root + "/")))))
          || (entry.type === "directory" && entry.path === undefined)
          || (entry.type === "document" && (!object(entry.info) || (Object.keys(entry.info).length !== 0 && !sameJson(entry.info, expected)))))) throw new Error("OpenCode did not confirm the isolated managed configuration sources");
    // Cold location registries are populated asynchronously. Wait for the exact
    // required built-in plumbing; never retry a failed or ambient plugin.
    const deadline = Date.now() + 15_000;
    while (true) {
      const plugins = this.locationData(await this.request(`/api/plugin${query}`));
      if (plugins.some((plugin) => !object(plugin) || !object(plugin.source) || plugin.source.type !== "builtin"
          || !object(plugin.state) || plugin.state.status !== "active" || typeof plugin.id !== "string"
          || V2_DISABLED_PLUGINS.includes(`-${plugin.id}`))) throw new Error("OpenCode managed plugin isolation failed");
      if (["opencode.config.agent", "opencode.config.mcp", "opencode.config.policy", "opencode.config.provider"].every((id) => plugins.some((plugin) => object(plugin) && plugin.id === id))) break;
      if (Date.now() >= deadline) throw new Error("OpenCode managed plugin activation timed out");
      await Bun.sleep(25);
    }
    const agents = this.locationData(await this.request(`/api/agent${query}`));
    const agent = agents.find((entry) => object(entry) && entry.id === AGENT);
    const rules = sessionPermissions(input.tools, this.version);
    if (agents.length !== 1 || !object(agent) || agent.mode !== "primary" || !Array.isArray(agent.permissions)
        || !sameJson(agent.permissions.slice(-rules.length), rules)) throw new Error("OpenCode resolved agent permissions differ from the managed allowlist");
    const mcp = this.locationData(await this.request(`/api/mcp${query}`));
    if (mcp.length !== 1 || !object(mcp[0]) || mcp[0].name !== "codemux" || !object(mcp[0].status) || mcp[0].status.status !== "connected") throw new Error("OpenCode did not connect to the private managed MCP server");
    const providers = this.locationData(await this.request(`/api/provider${query}`));
    const ref = modelRef(input);
    if (providers.length !== 1 || !providers.some((entry) => object(entry) && entry.id === ref.providerID
        && (entry.activation === "auto" || entry.activation === "enabled") && typeof entry.package === "string"
        && entry.package.startsWith("@opencode/ai/providers/"))) throw new Error("Managed OpenCode requires an available built-in provider; native OAuth and custom plugin routes are unavailable");
    const models = this.locationData(await this.request(`/api/model${query}`));
    const model = models.find((entry) => object(entry) && entry.providerID === ref.providerID && entry.id === ref.id && entry.enabled === true);
    if (!object(model) || (input.effort && (!Array.isArray(model.variants) || !model.variants.some((variant) => object(variant) && variant.id === input.effort)))) throw new Error("Managed OpenCode model or effort variant is unavailable");
    // v2 session projection treats an omitted variant as the implicit "default".
    this.selectedModel = ref;
    const created = await this.request("/api/session", { title: "CodeMux managed workflow", location: { directory: input.cwd }, agent: AGENT, model: this.selectedModel, permissions: rules });
    const session = this.assertSessionV2(object(created) ? created.data : undefined);
    const persisted = await this.request(`/api/session/${session.id}`);
    this.assertSessionV2(object(persisted) ? persisted.data : undefined, String(session.id));
    this.session = String(session.id);
    return { protocolVersion: 1, provider: "opencode", adapterVersion: OPENCODE_ADAPTER_VERSION, sessionId: this.session,
      tools: input.tools.map((tool) => tool.name), isolation: { nativeTools: [], nativeFanout: false, ambientConfig: false } };
  }

  private async pages(path: string, signal?: AbortSignal): Promise<Record<string, unknown>[]> {
    const result: Record<string, unknown>[] = []; const cursors = new Set<string>(); const ids = new Set<string>();
    let next: string | undefined; let bytes = 0;
    for (let page = 0; page < 128; page++) {
      const value = await this.request(next ? `${path.split("?")[0]}?limit=200&cursor=${encodeURIComponent(next)}` : path, undefined, signal);
      if (!object(value) || !Array.isArray(value.data) || !object(value.cursor)) throw new Error("OpenCode managed transcript page was malformed");
      bytes += Buffer.byteLength(JSON.stringify(value));
      if (bytes > MAX_RESPONSE) throw new Error("OpenCode managed history exceeded its limit");
      for (const item of value.data) {
        if (!object(item) || typeof item.id !== "string" || ids.has(item.id)) throw new Error("OpenCode managed transcript identity was malformed");
        ids.add(item.id); result.push(item);
      }
      if (value.cursor.next === undefined) return result;
      if (typeof value.cursor.next !== "string" || !value.cursor.next || value.cursor.next.length > 8192 || cursors.has(value.cursor.next)) throw new Error("OpenCode managed history cursor was malformed");
      next = value.cursor.next; cursors.add(next);
    }
    throw new Error("OpenCode managed history page limit exceeded");
  }

  private async turnV2(prompt: string, active: NonNullable<OpenCodeRuntime["active"]>): Promise<unknown[]> {
    const session = this.session!; const signal = active.abort.signal;
    const current = await this.request(`/api/session/${session}`, undefined, signal);
    this.assertSessionV2(object(current) ? current.data : undefined, session);
    const baseline = await this.pages(`/api/session/${session}/message?limit=200&order=asc`, signal);
    for (const message of baseline) this.knownMessages.add(String(message.id));
    if (active.cancelled) throw new Error("Managed turn was cancelled before admission");
    const id = `msg_${randomBytes(16).toString("hex")}`;
    active.admission = (async () => {
      const ack = await this.request(`/api/session/${session}/prompt`, { id, text: prompt, delivery: "queue", resume: true }, signal);
      if (!object(ack) || !object(ack.data) || ack.data.id !== id || ack.data.sessionID !== session
          || ack.data.type !== "user" || !object(ack.data.payload) || ack.data.payload.text !== prompt) throw new Error("OpenCode managed prompt admission was not confirmed");
    })();
    await active.admission;
    if (active.cancelled) throw new Error("Managed turn was cancelled after admission");
    await this.request(`/api/experimental/session/${session}/wait`, undefined, signal, "POST", true);
    const finished = await this.request(`/api/session/${session}`, undefined, signal);
    const state = this.assertSessionV2(object(finished) ? finished.data : undefined, session);
    if (!object(state.time) || typeof state.time.idle !== "number" || !Number.isSafeInteger(state.time.idle)
        || state.outcome !== "succeeded") throw new Error("OpenCode did not complete a successful managed turn");
    const children = await this.pages(`/api/session?directory=${encodeURIComponent(this.input!.cwd)}&parentID=${session}&limit=200&order=asc`, signal);
    if (children.length) throw new Error("OpenCode launched unmanaged child sessions");
    const messages = await this.pages(`/api/session/${session}/message?limit=200&order=asc`, signal);
    const fresh = messages.filter((message) => !this.knownMessages.has(String(message.id)));
    const user = fresh.findIndex((message) => message.type === "user" && message.id === id && message.text === prompt);
    const idle = fresh.at(-1);
    if (user < 0 || !idle || idle.type !== "idle" || idle.outcome !== "succeeded" || idle.error
        || fresh.slice(user + 1).some((message) => message.type === "user")) throw new Error("OpenCode managed transcript did not confirm this turn's outcome");
    const assistants = fresh.slice(user + 1).filter((message) => message.type === "assistant");
    if (!assistants.length || assistants.some((message) => message.error || !Array.isArray(message.content))) throw new Error("OpenCode did not complete a successful managed turn");
    const allowed = new Set(this.input!.tools.map((tool) => `codemux_${tool.name}`));
    for (const message of assistants) for (const part of message.content as unknown[]) {
      if (!object(part) || !["text", "reasoning", "tool"].includes(String(part.type))
          || (part.type === "tool" && (typeof part.name !== "string" || !allowed.has(part.name)))) throw new Error("OpenCode used an unmanaged native tool");
    }
    for (const message of fresh) if (message.type !== "assistant") this.knownMessages.add(String(message.id));
    return assistants.map((message) => ({ info: { id: message.id, role: "assistant", tokens: message.tokens }, parts: message.content }));
  }

  async startTurn(input: { turnId: string; prompt: string }): Promise<void> {
    if (this.disposed || !this.session || !this.input || this.active) throw new Error("Managed OpenCode turn is unavailable");
    const active: NonNullable<OpenCodeRuntime["active"]> = { id: input.turnId, abort: new AbortController(), cancelled: false };
    this.active = active;
    const started = Date.now(); const session = this.session; const config = this.input;
    void (async () => {
      let status = "error"; let usage: Json | undefined;
      try {
        let messages: unknown;
        if (this.version === "2.0.27") {
          messages = await this.turnV2(input.prompt, active);
          for (const message of messages as unknown[]) if (object(message) && Array.isArray(message.parts)) {
            for (const part of message.parts) if (!active.cancelled && object(part) && part.type === "text" && typeof part.text === "string") this.host.notify("text", { turnId: input.turnId, text: part.text });
          }
        } else {
          const slash = config.model!.indexOf("/");
          const result = await this.request(`/session/${session}/message`, {
            agent: AGENT, model: { providerID: config.model!.slice(0, slash), modelID: config.model!.slice(slash + 1) },
            ...(config.effort ? { variant: config.effort } : {}), parts: [{ type: "text", text: input.prompt }],
          }, active.abort.signal);
          if (!object(result) || !object(result.info) || result.info.role !== "assistant" || result.info.error || !Array.isArray(result.parts)) throw new Error("OpenCode did not complete a successful managed turn");
          const children = await this.request(`/session/${session}/children`);
          if (!Array.isArray(children) || children.length !== 0) throw new Error("OpenCode launched unmanaged child sessions");
          for (const part of result.parts) if (!active.cancelled && object(part) && part.type === "text" && typeof part.text === "string") this.host.notify("text", { turnId: input.turnId, text: part.text });
          messages = await this.request(`/session/${session}/message`);
        }
        if (!Array.isArray(messages)) throw new Error("OpenCode managed usage could not be reconciled");
        let turns = 0; let knownTokens = true;
        const tokens: Record<string, number> = { inputTokens: 0, outputTokens: 0, reasoningTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0 };
        for (const message of messages) {
          if (!object(message) || !object(message.info) || typeof message.info.id !== "string") throw new Error("OpenCode managed transcript was malformed");
          if (this.knownMessages.has(message.info.id)) continue;
          this.knownMessages.add(message.info.id);
          if (message.info.role !== "assistant") continue;
          turns++;
          const usage = nativeTokens(message.info.tokens);
          if (usage) {
            for (const [key, count] of Object.entries(usage)) {
              tokens[key] = (tokens[key] ?? 0) + count;
              if (!Number.isSafeInteger(tokens[key])) knownTokens = false;
            }
          } else knownTokens = false;
        }
        // Native info.cost is calculated from catalogue rates, not an invoice.
        // Keep observed counters but leave billed cost unknown.
        usage = { durationMs: Date.now() - started, numTurns: turns, model: config.model,
          ...(knownTokens && turns > 0 ? { tokens } : {}) };
        status = active.cancelled ? "cancelled" : "success";
      } catch { status = active.cancelled ? "cancelled" : "error"; }
      finally {
        try {
          await active.cancellation;
          // A cancellation can arrive while the accounting continuation yields.
          // It still owns this active turn until its terminal notification.
          if (active.cancelled) { await active.cancellation; status = "cancelled"; }
        } catch { /* Failed cancellation closes the owner below. */ }
        const notify = !this.disposed;
        // An invalid v2 outcome/admission must not leave an unobserved loop alive.
        if (notify && status === "error" && this.version === "2.0.27") await this.dispose();
        if (this.active === active) this.active = undefined;
        if (notify) this.host.notify("complete", { turnId: input.turnId, status,
          ...(usage ? { usage } : {}), ...(status === "error" ? { message: "Managed OpenCode turn failed; inspect the workflow attempt" } : {}) });
      }
    })();
  }

  async cancel(): Promise<void> {
    const active = this.active;
    if (!active || !this.session) return;
    active.cancelled = true;
    if (active.cancellation) return await active.cancellation;
    const session = this.session;
    active.cancellation = (async () => {
      try {
        if (this.version === "2.0.27") {
          // Idle interrupt/wait cannot fence a prompt still being admitted.
          // Confirm the dispatched admission before quiescing; an unknown ACK
          // rejects into verified owned-runtime teardown below.
          await active.admission;
          const ack = await this.request(`/api/session/${session}/interrupt?resume=false`, undefined, undefined, "POST");
          if (!object(ack) || typeof ack.interrupted !== "boolean") throw new Error("OpenCode did not acknowledge interruption");
          await this.request(`/api/experimental/session/${session}/wait`, undefined, undefined, "POST", true);
        } else await this.request(`/session/${session}/abort`, {});
      } catch (error) { await this.dispose(); throw error; }
      finally { active.abort.abort(); }
    })();
    await active.cancellation;
  }

  async dispose(): Promise<void> {
    if (this.disposed) return;
    this.disposed = true;
    this.active?.abort.abort();
    this.mcp?.stop();
    await this.native?.stop();
    // Rust owns stateDir and removes it only after verified process-group stop.
  }
}
