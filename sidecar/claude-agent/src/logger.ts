// Structured logger. Writes NEWLINE-DELIMITED JSON to stderr.
//
// CRITICAL: nothing in the sidecar should ever write to stdout except
// JSON-RPC envelopes. Stdout is the transport the Rust side parses —
// any stray line would be a protocol error. All logs therefore go to
// stderr, where the Rust side keeps a tail buffer for diagnostics but
// never parses the content.

type Fields = Record<string, unknown>;

function write(level: "info" | "warn" | "error", msg: string, fields?: Fields): void {
  const line = JSON.stringify({
    level,
    msg,
    ts: new Date().toISOString(),
    ...(fields ?? {}),
  });
  process.stderr.write(line + "\n");
}

export const logger = {
  info(msg: string, fields?: Fields): void {
    write("info", msg, fields);
  },
  warn(msg: string, fields?: Fields): void {
    write("warn", msg, fields);
  },
  error(msg: string, fields?: Fields): void {
    write("error", msg, fields);
  },
};
