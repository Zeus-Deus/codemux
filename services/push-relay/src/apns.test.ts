// @vitest-environment node
import { createServer } from "node:http2";
import type { AddressInfo } from "node:net";
import { describe, expect, it } from "vitest";
import { createHttp2Transport } from "./apns";

describe("createHttp2Transport", () => {
  it("POSTs over a reused HTTP/2 session and returns status and body", async () => {
    const seen: Array<{ path: string; topic: string; body: string; session: unknown }> = [];
    const server = createServer((req, res) => {
      let body = "";
      req.on("data", (chunk) => (body += chunk));
      req.on("end", () => {
        seen.push({ path: req.url, topic: String(req.headers["apns-topic"]), body, session: req.stream.session });
        res.writeHead(seen.length === 1 ? 200 : 410);
        res.end(seen.length === 1 ? "" : '{"reason":"Unregistered"}');
      });
    });
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    const origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
    const send = createHttp2Transport();
    try {
      expect(await send(`${origin}/3/device/abc`, { "apns-topic": "org.codemux.app" }, '{"a":1}')).toEqual({
        status: 200,
        body: "",
      });
      expect(await send(`${origin}/3/device/def`, {}, "{}")).toEqual({
        status: 410,
        body: '{"reason":"Unregistered"}',
      });
      expect(seen[0]).toMatchObject({ path: "/3/device/abc", topic: "org.codemux.app", body: '{"a":1}' });
      expect(seen[1].session).toBe(seen[0].session);
    } finally {
      for (const s of new Set(seen.map((x) => x.session as { destroy(): void }))) s.destroy();
      await new Promise((resolve) => server.close(resolve));
    }
  });
});
