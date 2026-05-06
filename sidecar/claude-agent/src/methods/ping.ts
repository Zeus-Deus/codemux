// `ping` — liveness probe. The only method the scaffold implements.
//
// Returns the input back verbatim along with `pong: true` and the
// server's current wall-clock time (ISO-8601). Useful as an end-to-end
// proof that the whole pipeline — Rust spawning, JSON-RPC framing,
// sidecar dispatch — works before any real methods are added.

export type PingParams = Record<string, unknown>;

export interface PingResult {
  pong: true;
  echo: PingParams;
  server_time: string;
}

export async function ping(params: PingParams): Promise<PingResult> {
  return {
    pong: true,
    echo: params,
    server_time: new Date().toISOString(),
  };
}
