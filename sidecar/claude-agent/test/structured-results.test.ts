import { afterEach, expect, test } from "bun:test";
import { buildCodemuxMcpServer, setMcpBridgeWriter } from "../src/mcp-bridge.ts";
import { createPluginPermissions } from "../src/plugin-permissions.ts";
import { dispatchResponse, _resetUpstreamRpcForTests } from "../src/upstream-rpc.ts";
import { callSdk } from "./sdk-mcp-client.ts";

afterEach(() => _resetUpstreamRpcForTests());
for (const bridge of [false,true]) {
  test(`Claude SDK preserves text, image and structuredContent (bridge=${bridge})`,async () => {
    const fixture = await Bun.file(new URL("../../../src-tauri/tests/fixtures/plugin_mcp_result.json", import.meta.url)).json();
    const guard = createPluginPermissions({
      threadId:"structured",getQuery:() => ({getSettings:async () => ({effective:{permissions:{}}})}),
      getMode:() => "bypassPermissions",canUseTool:async () => ({behavior:"allow"}),canUseTarget:async () => ({behavior:"allow"}),
    });
    const args = bridge ? {name:"fixture__image",arguments:{}} : {};
    const name = bridge ? "codemux_call_plugin_tool" : "fixture__image";
    if (bridge) await guard.hook({hook_event_name:"PreToolUse",tool_name:"mcp__codemux__codemux_call_plugin_tool",tool_input:args,tool_use_id:"structured",session_id:"s",transcript_path:"/fixture",cwd:"/fixture"},"structured",{signal:new AbortController().signal});
    const calls: string[] = [];
    setMcpBridgeWriter((line) => {
      calls.push(line);
      const request = JSON.parse(line);
      queueMicrotask(() => dispatchResponse({jsonrpc:"2.0",id:request.id,result:fixture}));
    });
    const server = buildCodemuxMcpServer([{name,prefixedName:name,serverId:bridge?"codemux-plugin-bridge":"plugin:test:fixture",description:name,inputSchema:{type:"object",properties:{name:{type:"string"},arguments:{type:"object"}}}}],guard);
    const response = await callSdk(server,name,args,"structured");
    expect(response).toEqual({jsonrpc:"2.0",id:2,result:fixture});
    expect(JSON.stringify(response).split("STRUCTURED-581")).toHaveLength(2);
    expect(calls).toHaveLength(1);
  });
}
