import { afterEach, expect, test } from "bun:test";
import type { SDKMessage } from "@anthropic-ai/claude-agent-sdk";
import { ClaudeSession, setQueryFactoryForTests, resetQueryFactoryForTests } from "../src/session.ts";
import type { buildCodemuxMcpServer, RegisteredMcpTool } from "../src/mcp-bridge.ts";
import { FakeQuery, asQuery } from "./fake-query.ts";

type SdkServer = ReturnType<typeof buildCodemuxMcpServer>;
function sdkServer(value: unknown): value is SdkServer {
  return typeof value === "object" && value !== null && "type" in value && value.type === "sdk" && "instance" in value;
}
async function list(server: SdkServer | undefined) {
  if (!server) throw new Error("codemux is disconnected");
  let finish!: (value: unknown) => void;
  const response = new Promise<unknown>((resolve) => { finish = resolve; });
  const transport: Parameters<typeof server.instance.connect>[0] = {
    start: async () => {}, close: async () => {},
    send: async (message) => { if ("id" in message && message.id === 2) finish(message); },
  };
  await server.instance.connect(transport);
  try {
    transport.onmessage!({jsonrpc:"2.0",id:1,method:"initialize",params:{protocolVersion:"2024-11-05",capabilities:{},clientInfo:{name:"lifecycle-test",version:"1"}}});
    await tick();
    transport.onmessage!({jsonrpc:"2.0",method:"notifications/initialized"});
    transport.onmessage!({jsonrpc:"2.0",id:2,method:"tools/list",params:{}});
    return JSON.stringify(await response);
  } finally { await server.instance.close(); }
}
const tick = () => new Promise<void>((resolve) => setTimeout(resolve, 0));
const tool = (name: string, serverId: string): RegisteredMcpTool => ({name,prefixedName:name,serverId,description:name,inputSchema:{type:"object"}});
const plugin = [tool("fixture__window_state","plugin:test:fixture"),tool("codemux_list_plugin_tools","codemux-plugin-bridge"),tool("codemux_call_plugin_tool","codemux-plugin-bridge")];
afterEach(() => resetQueryFactoryForTests());

for (const keepHost of [false, true]) {
  for (const timing of ["between turns", "during a turn", "during removal"]) {
    test(`last plugin re-registers ${timing} (host tools=${keepHost})`, async () => {
      let fake!: FakeQuery;
      setQueryFactoryForTests((args) => { fake = new FakeQuery(args); return asQuery(fake); });
      const host = keepHost ? [tool("mcp__codemux__echo","codemux-host")] : [];
      const tools = [...host,...plugin];
      const session = new ClaudeSession({threadId:"lifecycle",cwd:"/fixture",pathToClaudeCodeExecutable:"/fixture/claude",mcpTools:tools,permissionMode:"bypassPermissions"},{notification() {}});
      const originalQuery = fake;
      let active = fake.capturedOptions?.mcpServers?.codemux;
      const originalServer = active;
      let release: (() => void) | undefined;
      let pauseRemoval = timing === "during removal";
      fake.setMcpServers = async (servers) => {
        fake.setMcpServersCalls.push(Object.keys(servers));
        active = sdkServer(servers.codemux) ? servers.codemux : undefined;
        if (pauseRemoval && !servers.codemux) {
          pauseRemoval = false;
          await new Promise<void>((resolve) => { release = resolve; });
        }
        return {added:Object.keys(servers),removed:[],errors:{}};
      };
      try {
        if (!sdkServer(active)) throw new Error("missing initial server");
        expect(await list(active)).toContain("fixture__window_state");
        if (timing === "during a turn") await session.sendTurn({text:"first"});
        if (timing === "between turns") {
          await session.sendTurn({text:"first"});
          fake.emit({type:"result",subtype:"success",session_id:"same-session",result:"done"} as SDKMessage);
          await tick();
          await session.updateMcpTools(host);
          if (sdkServer(active)) expect(await list(active)).not.toContain("fixture__window_state");
          else expect(keepHost).toBe(false);
          await session.updateMcpTools(tools);
        } else {
          const removed = session.updateMcpTools(host);
          const registered = session.updateMcpTools(tools);
          release?.();
          await Promise.all([removed,registered]);
        }
        if (timing === "during a turn") {
          expect(fake.setMcpServersCalls).toEqual([]);
          fake.emit({type:"result",subtype:"success",session_id:"same-session",result:"done"} as SDKMessage);
          await tick();
        }
        await session.sendTurn({text:"next turn: list the tools"});
        if (!sdkServer(active)) throw new Error("codemux disconnected after re-register");
        const listed = await list(active);
        for (const item of plugin) expect(listed).toContain(item.prefixedName);
        if (keepHost) expect(listed).toContain("mcp__codemux__echo");
        expect(fake).toBe(originalQuery);
        expect(active).not.toBe(originalServer);
        expect(fake.closed).toBe(false);
        expect(fake.setPermissionModeCalls).toEqual([]);
      } finally { release?.(); await session.close(); }
    });
  }
}
