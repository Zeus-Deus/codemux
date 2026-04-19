// End-to-end tests for the sidecar's `ping` method.
//
// Each test spawns `bun run src/main.ts` as a subprocess, exchanges
// JSON-RPC messages over stdio, and asserts the response shape. The
// compiled binary is exercised separately from the Rust side (see
// `src-tauri/tests/sidecar_ping.rs`) — running these via `bun run` keeps
// the test loop fast.

import { describe, expect, test } from "bun:test";
import { join, resolve } from "node:path";

const projectRoot = resolve(import.meta.dir, "..");
const entryPoint = join(projectRoot, "src", "main.ts");

interface SpawnedSidecar {
  proc: ReturnType<typeof Bun.spawn>;
  writeLine: (obj: unknown) => Promise<void>;
  readLines: () => AsyncGenerator<string>;
  readNextJson: () => Promise<unknown>;
  readStderr: () => Promise<string>;
  close: () => Promise<void>;
}

/** Spawn the sidecar with its source-mode entry point and expose a
 *  stdin writer + stdout line iterator. Caller is responsible for
 *  calling `close()`. */
function spawnSidecar(): SpawnedSidecar {
  const proc = Bun.spawn(["bun", "run", entryPoint], {
    stdin: "pipe",
    stdout: "pipe",
    stderr: "pipe",
    cwd: projectRoot,
  });

  const stdin = proc.stdin as unknown as { write: (chunk: string) => number };
  const writer = stdin;

  async function writeLine(obj: unknown): Promise<void> {
    writer.write(JSON.stringify(obj) + "\n");
  }

  async function* readLines(): AsyncGenerator<string> {
    const reader = (proc.stdout as ReadableStream<Uint8Array>).getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    try {
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        while (true) {
          const nl = buffer.indexOf("\n");
          if (nl === -1) break;
          yield buffer.slice(0, nl);
          buffer = buffer.slice(nl + 1);
        }
      }
      if (buffer.trim() !== "") yield buffer;
    } finally {
      reader.releaseLock();
    }
  }

  const lineIter = readLines();

  async function readNextJson(): Promise<unknown> {
    const { value, done } = await lineIter.next();
    if (done) throw new Error("stdout closed before a line arrived");
    return JSON.parse(value);
  }

  async function readStderr(): Promise<string> {
    const r = (proc.stderr as ReadableStream<Uint8Array>).getReader();
    const decoder = new TextDecoder();
    let buf = "";
    try {
      while (true) {
        const { value, done } = await r.read();
        if (done) break;
        buf += decoder.decode(value, { stream: true });
      }
    } finally {
      r.releaseLock();
    }
    return buf;
  }

  async function close(): Promise<void> {
    try {
      (proc.stdin as unknown as { end: () => void }).end();
    } catch {
      // ignore: already closed
    }
    try {
      await proc.exited;
    } catch {
      // ignore
    }
  }

  return { proc, writeLine, readLines, readNextJson, readStderr, close };
}

/** Send one request, read one response, then close. */
async function oneshot(request: unknown): Promise<unknown> {
  const s = spawnSidecar();
  await s.writeLine(request);
  const resp = await s.readNextJson();
  await s.close();
  return resp;
}

describe("ping", () => {
  test("ping with empty params returns pong=true", async () => {
    const resp = (await oneshot({
      jsonrpc: "2.0",
      id: 1,
      method: "ping",
      params: {},
    })) as {
      jsonrpc: string;
      id: number;
      result: { pong: boolean; echo: unknown; server_time: string };
    };
    expect(resp.jsonrpc).toBe("2.0");
    expect(resp.id).toBe(1);
    expect(resp.result.pong).toBe(true);
    expect(resp.result.echo).toEqual({});
    // ISO-8601 (with optional fractional seconds and Z suffix).
    expect(resp.result.server_time).toMatch(
      /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z$/,
    );
  });

  test("ping with object params echoes them back", async () => {
    const resp = (await oneshot({
      jsonrpc: "2.0",
      id: "abc",
      method: "ping",
      params: { foo: "bar", n: 7 },
    })) as { id: string; result: { echo: Record<string, unknown> } };
    expect(resp.id).toBe("abc");
    expect(resp.result.echo).toEqual({ foo: "bar", n: 7 });
  });
});

describe("protocol errors", () => {
  test("unknown method returns -32601", async () => {
    const resp = (await oneshot({
      jsonrpc: "2.0",
      id: 42,
      method: "not_a_real_method",
      params: {},
    })) as { id: number; error: { code: number; message: string } };
    expect(resp.id).toBe(42);
    expect(resp.error.code).toBe(-32601);
    expect(resp.error.message).toContain("not_a_real_method");
  });

  test("malformed JSON returns -32700 with offending line in data", async () => {
    const s = spawnSidecar();
    (s.proc.stdin as unknown as { write: (c: string) => number }).write(
      "this is not json\n",
    );
    const resp = (await s.readNextJson()) as {
      id: null;
      error: { code: number; message: string; data: { line: string } };
    };
    await s.close();
    expect(resp.id).toBeNull();
    expect(resp.error.code).toBe(-32700);
    expect(resp.error.data.line).toContain("this is not json");
  });

  test("notification (no id) does not produce a response", async () => {
    const s = spawnSidecar();
    // Send a notification — no `id` field — then follow up with a real
    // request whose response should be the FIRST line on stdout.
    await s.writeLine({ jsonrpc: "2.0", method: "ping", params: { tag: 1 } });
    await s.writeLine({
      jsonrpc: "2.0",
      id: 99,
      method: "ping",
      params: { tag: 2 },
    });
    const first = (await s.readNextJson()) as { id: number; result: { echo: { tag: number } } };
    await s.close();
    expect(first.id).toBe(99);
    expect(first.result.echo.tag).toBe(2);
  });
});

describe("lifecycle", () => {
  test("multiple concurrent requests all complete", async () => {
    const s = spawnSidecar();
    const N = 10;
    for (let i = 0; i < N; i++) {
      await s.writeLine({
        jsonrpc: "2.0",
        id: i,
        method: "ping",
        params: { i },
      });
    }
    const got = new Set<number>();
    for (let i = 0; i < N; i++) {
      const r = (await s.readNextJson()) as {
        id: number;
        result: { echo: { i: number } };
      };
      expect(r.result.echo.i).toBe(r.id);
      got.add(r.id);
    }
    await s.close();
    expect(got.size).toBe(N);
  });

  test("stdin EOF causes clean exit with code 0", async () => {
    const s = spawnSidecar();
    await s.writeLine({ jsonrpc: "2.0", id: 1, method: "ping", params: {} });
    await s.readNextJson();
    // Close stdin; sidecar should exit cleanly.
    (s.proc.stdin as unknown as { end: () => void }).end();
    const code = await s.proc.exited;
    expect(code).toBe(0);
  });

  test("SIGTERM causes clean exit with code 0", async () => {
    const s = spawnSidecar();
    // Give main() a beat to install signal handlers.
    await s.writeLine({ jsonrpc: "2.0", id: 1, method: "ping", params: {} });
    await s.readNextJson();
    s.proc.kill("SIGTERM");
    const code = await s.proc.exited;
    expect(code).toBe(0);
  });
});

describe("stdio discipline", () => {
  test("stderr logs are valid JSON lines", async () => {
    const s = spawnSidecar();
    await s.writeLine({ jsonrpc: "2.0", id: 1, method: "ping", params: {} });
    await s.readNextJson();
    (s.proc.stdin as unknown as { end: () => void }).end();
    await s.proc.exited;
    const stderr = await s.readStderr();
    const lines = stderr.split("\n").filter((l) => l.trim() !== "");
    // Every stderr line must be parseable JSON and carry the expected
    // fields. This prevents anyone from sneaking a `console.log` into
    // the code path.
    for (const line of lines) {
      const parsed = JSON.parse(line) as {
        level: string;
        msg: string;
        ts: string;
      };
      expect(parsed.level).toMatch(/^(info|warn|error)$/);
      expect(typeof parsed.msg).toBe("string");
      expect(typeof parsed.ts).toBe("string");
    }
    // There should be at least "sidecar started" and "stdin closed,
    // exiting" — proving we saw the full lifecycle and all of it
    // stayed on stderr.
    expect(lines.length).toBeGreaterThanOrEqual(2);
  });

  test("stdout contains ONLY valid JSON-RPC — no log bleed", async () => {
    const s = spawnSidecar();
    // Hit multiple code paths: happy request, notification, bad
    // method, malformed json. None should produce a non-JSON stdout
    // line.
    await s.writeLine({ jsonrpc: "2.0", id: 1, method: "ping", params: {} });
    await s.writeLine({ jsonrpc: "2.0", method: "ping", params: {} });
    await s.writeLine({ jsonrpc: "2.0", id: 2, method: "no_such" });
    (s.proc.stdin as unknown as { write: (c: string) => number }).write(
      "not-json\n",
    );
    await s.writeLine({ jsonrpc: "2.0", id: 3, method: "ping", params: {} });

    const seen: unknown[] = [];
    for (let i = 0; i < 4; i++) {
      seen.push(await s.readNextJson());
    }
    (s.proc.stdin as unknown as { end: () => void }).end();
    await s.proc.exited;

    // Every captured line must have jsonrpc: "2.0".
    for (const item of seen) {
      const env = item as { jsonrpc: string };
      expect(env.jsonrpc).toBe("2.0");
    }
  });

  test("partial-line writes across chunk boundaries reassemble correctly", async () => {
    const s = spawnSidecar();
    // Split one logical JSON-RPC frame into several partial writes.
    const frame = JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "ping",
      params: { chunked: true },
    }) + "\n";
    const writer = s.proc.stdin as unknown as { write: (c: string) => number };
    const half = Math.floor(frame.length / 2);
    writer.write(frame.slice(0, half));
    // Force the runtime to schedule another IO tick between writes.
    await new Promise((r) => setTimeout(r, 10));
    writer.write(frame.slice(half));

    const resp = (await s.readNextJson()) as {
      id: number;
      result: { echo: { chunked: boolean } };
    };
    expect(resp.id).toBe(1);
    expect(resp.result.echo.chunked).toBe(true);
    await s.close();
  });
});
