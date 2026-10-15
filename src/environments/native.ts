/**
 * Desktop adapters for reaching the account's other environments.
 *
 * The browser's account client (`DeviceRegistry`) and iroh socket
 * (`IrohWebSocket`) already implement the device list, connect grants, the
 * account handshake and the `/ws` protocol. On desktop two seams are swapped:
 *
 * - {@link nativeAccountFetch}: device-registry requests go through Rust, which
 *   attaches the desktop's account token. The token never enters the webview.
 * - {@link NativeIrohDialer}: dials with the native iroh client, which can
 *   hole-punch a direct path instead of relaying every byte.
 */
import { Channel, invoke } from "@tauri-apps/api/core";
import type { IrohByteStream, IrohDialer } from "@/remote/iroh-transport";

interface AccountResponse {
  status: number;
  body: unknown;
}

/** A `fetch` for `DeviceRegistry` that proxies the account device endpoints. */
export const nativeAccountFetch = (async (
  input: RequestInfo | URL,
  init?: RequestInit,
): Promise<Response> => {
  const url = new URL(String(input instanceof Request ? input.url : input));
  const method = (init?.method ?? "GET").toUpperCase();
  const body = typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
  const response = await invoke<AccountResponse>("environment_account_request", {
    request: { method, path: url.pathname, body },
  });
  return new Response(JSON.stringify(response.body), {
    status: response.status,
    headers: { "Content-Type": "application/json" },
  });
}) as typeof fetch;

/** End marker the backend sends after the last chunk. */
interface StreamEnd {
  t: "end";
  error: string | null;
}

function isStreamEnd(message: unknown): message is StreamEnd {
  return (
    typeof message === "object" &&
    message !== null &&
    (message as { t?: unknown }).t === "end"
  );
}

interface Waiter {
  resolve(chunk: Uint8Array | null): void;
  reject(error: Error): void;
}

/** One native bi-stream. Chunks pushed by the backend wait in a queue until
 *  `read` pulls them; the end marker resolves pending and later reads with
 *  `null` (clean end) or rejects them (read error). */
export class NativeByteStream implements IrohByteStream {
  private readonly chunks: Uint8Array[] = [];
  private readonly waiters: Waiter[] = [];
  private ended: { error: string | null } | null = null;
  private closed = false;
  id: number | null = null;

  /** Feed a channel message from the backend. */
  push(message: unknown): void {
    if (this.ended) return;
    if (isStreamEnd(message)) {
      this.ended = { error: message.error };
      for (const waiter of this.waiters.splice(0)) this.settle(waiter);
      return;
    }
    const chunk =
      message instanceof ArrayBuffer
        ? new Uint8Array(message)
        : message instanceof Uint8Array
          ? message
          : Array.isArray(message)
            ? Uint8Array.from(message as number[])
            : null;
    if (!chunk) return;
    const waiter = this.waiters.shift();
    if (waiter) waiter.resolve(chunk);
    else this.chunks.push(chunk);
  }

  read(): Promise<Uint8Array | null> {
    const chunk = this.chunks.shift();
    if (chunk) return Promise.resolve(chunk);
    return new Promise((resolve, reject) => {
      const waiter = { resolve, reject };
      if (this.ended || this.closed) this.settle(waiter);
      else this.waiters.push(waiter);
    });
  }

  async write(data: Uint8Array): Promise<void> {
    if (this.id === null || this.closed) throw new Error("environment stream closed");
    await invoke("environment_stream_write", data, {
      headers: { "x-stream-id": String(this.id) },
    });
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    for (const waiter of this.waiters.splice(0)) waiter.resolve(null);
    if (this.id !== null) {
      void invoke("environment_stream_close", { streamId: this.id }).catch(() => {});
    }
  }

  private settle(waiter: Waiter): void {
    if (this.ended?.error) waiter.reject(new Error(this.ended.error));
    else waiter.resolve(null);
  }
}

export class NativeIrohDialer implements IrohDialer {
  async dial(target: { nodeId: string; relayUrl?: string | null }): Promise<IrohByteStream> {
    const stream = new NativeByteStream();
    const onData = new Channel<unknown>((message) => stream.push(message));
    stream.id = await invoke<number>("environment_dial", {
      nodeId: target.nodeId,
      relayUrl: target.relayUrl ?? null,
      onData,
    });
    return stream;
  }
}
