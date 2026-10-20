// Copy this folder to the plugin folder as mcp-tools, and set fixturePath
// to the absolute path of src-tauri/tests/fixtures/plugin_mcp.py.
import { PALETTE_AREA } from "@codemux/sdk";
const fixturePath = "/absolute/path/to/codemux/src-tauri/tests/fixtures/plugin_mcp.py";
export default {
  id: "mcp-tools",
  name: "MCP tools example",
  version: "0.1.0",
  async register(ctx) {
    await ctx.registerMcpServer({
      id: "fixture", name: "MCP fixture", toolPrefix: "fixture", scope: "all",
      transport: "stdio", command: "python3", args: [fixturePath],
    });
    ctx.register({
      id: "inspect", area: PALETTE_AREA,
      data: { label: "MCP example: inspect tools", run: async () => {
        const tools = await ctx.host.mcp.listTools("fixture");
        ctx.host.notify({ message: `MCP example: ${tools.length} tools` });
      } },
    });
    ctx.host.mcp.onActivity("fixture", (event) => console.log("MCP example activity", event));
  },
};
