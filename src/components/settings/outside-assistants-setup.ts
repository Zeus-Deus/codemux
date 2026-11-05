/**
 * The copy-ready setup commands Settings → Outside assistants prints, kept
 * pure so the outside-agent guide can be checked against them word for word.
 */

export type LocalClientId = "claude-code" | "codex" | "hermes" | "other";

export interface LocalSetupRow {
  id: LocalClientId;
  label: string;
  command: string;
  copyLabel: string;
}

/** Characters a POSIX shell never splits or expands. */
const SHELL_SAFE = /^[A-Za-z0-9_@%+=:,./-]+$/;
/** `C:\…`, `C:/…` or a `\\server\share` path. */
const WINDOWS_PATH = /^(?:[A-Za-z]:[\\/]|\\\\)/;

/** One shell word. Safe characters pass through; Windows-looking paths get
 *  double quotes; everything else gets POSIX single quotes ('\'' for an
 *  embedded quote). */
export function shellArg(value: string): string {
  if (SHELL_SAFE.test(value)) return value;
  if (WINDOWS_PATH.test(value)) return `"${value}"`;
  return `'${value.split("'").join("'\\''")}'`;
}

/** The four local commands, in display order. `executable` is the raw durable
 *  path to `codemux`. Each names its client with `--client`, which only labels
 *  it in attribution; Hermes also starts Supervised because it relays chat
 *  messages from other people. */
export function localSetupCommands(executable: string): LocalSetupRow[] {
  const codemux = shellArg(executable);
  return [
    {
      id: "claude-code",
      label: "Claude Code",
      command: `claude mcp add --scope user codemux -- ${codemux} mcp --client claude-code`,
      copyLabel: "Copy Claude Code command",
    },
    {
      id: "codex",
      label: "Codex",
      command: `codex mcp add codemux -- ${codemux} mcp --client codex`,
      copyLabel: "Copy Codex command",
    },
    {
      id: "hermes",
      label: "Hermes",
      command: `hermes mcp add codemux --command ${codemux} --args mcp --client hermes --access supervised`,
      copyLabel: "Copy Hermes command",
    },
    {
      id: "other",
      label: "Other",
      command: `${codemux} mcp --client my-agent`,
      copyLabel: "Copy command",
    },
  ];
}

/** Serve the loopback listener on this machine's tailnet HTTPS name. */
export function tailscaleServeCommand(port: number): string {
  return `tailscale serve --bg ${port}`;
}

/** Browser timers overflow above 2^31-1 ms and fire at once; clamp so a
 *  long-lived grant cannot spin reloads. */
export const MAX_TIMER_DELAY_MS = 2_147_483_647;

/** Milliseconds until the soonest of `expiries` (0 when one is already past),
 *  clamped to the timer range, or null when none is a valid time. */
export function expiryTimerDelay(expiries: readonly string[], now: number): number | null {
  const soonest = Math.min(...expiries.map((value) => Date.parse(value)).filter(Number.isFinite));
  if (!Number.isFinite(soonest)) return null;
  return Math.min(MAX_TIMER_DELAY_MS, Math.max(0, soonest - now));
}
