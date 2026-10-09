import { expect, test } from "bun:test";
import { Rpc } from "../src/rpc";
import type { Json } from "../src/contract";

test("host tool responses can complete a still-pending provider request", async () => {
  const lines: string[] = []; const rpc = new Rpc((line) => lines.push(line));
  const request = rpc.accept(JSON.stringify({ jsonrpc: "2.0", id: 1, method: "startTurn", params: {} }), async () => {
    return await rpc.request("managed/tool_call", { name: "workflow_result", arguments: {} });
  });
  const callback: { id: string } = JSON.parse(lines[0] ?? "null");
  await rpc.accept(JSON.stringify({ jsonrpc: "2.0", id: callback.id, result: { accepted: true } }), async (): Promise<Json> => null);
  await request;
  expect(JSON.parse(lines[1] ?? "null")).toEqual({ jsonrpc: "2.0", id: 1, result: { accepted: true } });
  rpc.close();
});

test("EOF revokes outstanding tool calls and unsolicited responses are fatal", async () => {
  const rpc = new Rpc(() => {});
  const pending = rpc.request("managed/tool_call", {});
  rpc.close();
  await expect(pending).rejects.toThrow("transport is closed");
  await expect(new Rpc(() => {}).accept('{"jsonrpc":"2.0","id":"forged","result":{}}', async () => null)).rejects.toThrow("Unexpected");
});

test("bounds messages and in-flight tool requests", async () => {
  const rpc = new Rpc(() => {}); const pending: Array<Promise<Json>> = [];
  for (let i = 0; i < 64; i++) pending.push(rpc.request("managed/tool_call", {}));
  await expect(rpc.request("managed/tool_call", {})).rejects.toThrow("unavailable");
  await expect(rpc.accept("x".repeat(2 * 1024 * 1024 + 1), async () => null)).rejects.toThrow("size limit");
  rpc.close(); await Promise.allSettled(pending);
});
