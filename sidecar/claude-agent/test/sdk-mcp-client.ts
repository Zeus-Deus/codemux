import type { buildCodemuxMcpServer } from "../src/mcp-bridge.ts";

export async function callSdk(
  server: ReturnType<typeof buildCodemuxMcpServer>,
  name: string,
  args: Record<string, unknown>,
  toolUseId = "call",
  onResponse?: () => void,
) {
  let finish!: (value: unknown) => void;
  const result = new Promise<unknown>((resolve) => {
    finish = resolve;
  });
  const transport: Parameters<typeof server.instance.connect>[0] = {
    start: async () => {},
    close: async () => {},
    send: async (message) => {
      if ("id" in message && message.id === 2) {
        onResponse?.();
        finish(message);
      }
    },
  };
  await server.instance.connect(transport);
  transport.onmessage!({
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: {
      protocolVersion: "2024-11-05",
      capabilities: {},
      clientInfo: { name: "test", version: "1" },
    },
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  transport.onmessage!({ jsonrpc: "2.0", method: "notifications/initialized" });
  transport.onmessage!({
    jsonrpc: "2.0",
    id: 2,
    method: "tools/call",
    params: {
      name,
      arguments: args,
      _meta: { "claudecode/toolUseId": toolUseId },
    },
  });
  try {
    return await result;
  } finally {
    await server.instance.close();
  }
}
