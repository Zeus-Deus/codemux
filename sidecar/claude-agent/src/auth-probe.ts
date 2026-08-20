// Subprocess probes for the user's local `claude` binary.
//
// This file is deliberately isolated — it is the only place in the
// sidecar allowed to spawn `claude` directly (for `--version` and
// `auth status`). All Claude *inference* goes through the SDK's
// `query()`. `scripts/check-tos-boundary.sh` allow-lists this file
// for the `spawn/exec claude` pattern and nothing else should be.

import { spawn } from "node:child_process";

const PROBE_TIMEOUT_MS = 5_000;

/** Substrings that indicate the CLI is installed but the user has
 *  not logged in. Case-insensitive match against combined
 *  stdout+stderr output. */
const UNAUTHENTICATED_PATTERNS = [
  "not logged in",
  "run claude login",
  "not authenticated",
  // Must be listed (and therefore matched) BEFORE the positive
  // heuristic below, which looks for the substring "authenticated" —
  // that substring also sits inside "unauthenticated", so a CLI that
  // reports `Status: unauthenticated` would otherwise be read as
  // logged in and no banner would ever appear.
  "unauthenticated",
  "please log in",
  "login required",
];

/** Outcome of `probeInstalled`. */
export interface ProbeInstalledResult {
  installed: boolean;
  /** CLI-reported version string, when available. */
  version?: string;
}

/** Outcome of `probeAuthenticated`. */
export interface ProbeAuthenticatedResult {
  status: "authenticated" | "unauthenticated" | "unknown";
  /** User-visible hint or raw output snippet. */
  message?: string;
}

interface SpawnOutcome {
  stdout: string;
  stderr: string;
  exitCode: number | null;
  error?: Error;
  timedOut: boolean;
}

/** Spawn a process, capture stdio, enforce a wall-clock timeout. */
function runBinary(binary: string, args: string[]): Promise<SpawnOutcome> {
  return new Promise<SpawnOutcome>((resolve) => {
    const child = spawn(binary, args, {
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill("SIGKILL");
    }, PROBE_TIMEOUT_MS);

    child.stdout?.on("data", (chunk: Buffer) => {
      stdout += chunk.toString("utf8");
    });
    child.stderr?.on("data", (chunk: Buffer) => {
      stderr += chunk.toString("utf8");
    });
    child.on("error", (error: Error) => {
      clearTimeout(timer);
      resolve({ stdout, stderr, exitCode: null, error, timedOut });
    });
    child.on("close", (code: number | null) => {
      clearTimeout(timer);
      resolve({ stdout, stderr, exitCode: code, timedOut });
    });
  });
}

/** Run `<binary> --version` and parse. */
export async function probeInstalled(
  binary: string,
): Promise<ProbeInstalledResult> {
  const result = await runBinary(binary, ["--version"]);
  if (result.timedOut || result.error || result.exitCode !== 0) {
    return { installed: false };
  }
  const trimmed = result.stdout.trim();
  if (trimmed === "") {
    return { installed: true, version: "unknown" };
  }
  // Typical output: "claude 1.2.3" or "Claude Code 2.1.114".
  const firstLine = trimmed.split("\n")[0] ?? trimmed;
  const token = firstLine.split(/\s+/).pop() ?? firstLine;
  return { installed: true, version: token };
}

/** Newer CLIs print `auth status` as a JSON object with a `loggedIn`
 *  boolean. The legacy substring matcher below never sees the phrase
 *  "logged in" in that form (the key is `loggedIn`), so without this
 *  step a fully logged-in user is reported as `unknown` forever and
 *  the chat surface shows a "could not verify" banner that no amount
 *  of re-login clears. Returns `null` when the output is not JSON or
 *  lacks the field, so the text heuristics still apply. */
function classifyJsonAuthOutput(
  output: string,
): ProbeAuthenticatedResult | null {
  for (const candidate of jsonObjectCandidates(output)) {
    let parsed: unknown;
    try {
      parsed = JSON.parse(candidate);
    } catch {
      continue;
    }
    if (typeof parsed !== "object" || parsed === null) continue;
    const loggedIn = (parsed as Record<string, unknown>)["loggedIn"];
    if (typeof loggedIn !== "boolean") continue;
    return loggedIn
      ? { status: "authenticated" }
      : {
          status: "unauthenticated",
          message:
            "Claude CLI is not authenticated. Run `claude login` and retry.",
        };
  }
  return null;
}

/** Every balanced top-level `{...}` region in `output`, in order.
 *
 *  Scanning for balanced regions rather than slicing first-`{` to
 *  last-`}` matters because the classified text is stdout AND stderr
 *  concatenated: one brace-bearing extra line (an update notice, a
 *  second JSON object, a runtime warning) would make the single wide
 *  slice unparseable and silently drop the whole probe back to
 *  `unknown` — reinstating the very "could not verify" banner this
 *  parsing exists to prevent. Quoted strings are tracked so braces
 *  inside values (an org name, say) do not unbalance the scan. */
function* jsonObjectCandidates(output: string): Generator<string> {
  let depth = 0;
  let start = -1;
  let inString = false;
  let escaped = false;
  for (let i = 0; i < output.length; i += 1) {
    const ch = output[i];
    if (inString) {
      if (escaped) escaped = false;
      else if (ch === "\\") escaped = true;
      else if (ch === '"') inString = false;
      continue;
    }
    if (ch === '"') {
      inString = true;
    } else if (ch === "{") {
      if (depth === 0) start = i;
      depth += 1;
    } else if (ch === "}") {
      // A stray closer outside any object is noise, not an underflow.
      if (depth === 0) continue;
      depth -= 1;
      if (depth === 0 && start !== -1) {
        yield output.slice(start, i + 1);
        start = -1;
      }
    }
  }
}

/** Classify combined stdout+stderr into an auth status. Split out
 *  so it is pure and unit-testable. */
export function classifyAuthOutput(output: string): ProbeAuthenticatedResult {
  const structured = classifyJsonAuthOutput(output);
  if (structured) return structured;
  const lower = output.toLowerCase();
  for (const pattern of UNAUTHENTICATED_PATTERNS) {
    if (lower.includes(pattern)) {
      return {
        status: "unauthenticated",
        message:
          "Claude CLI is not authenticated. Run `claude login` and retry.",
      };
    }
  }
  if (lower.includes("logged in") || lower.includes("authenticated")) {
    return { status: "authenticated" };
  }
  return {
    status: "unknown",
    message: output.trim().slice(0, 500),
  };
}

/** Run `<binary> auth status` and parse. */
export async function probeAuthenticated(
  binary: string,
): Promise<ProbeAuthenticatedResult> {
  const result = await runBinary(binary, ["auth", "status"]);
  if (result.timedOut) {
    return { status: "unknown", message: "probe timed out" };
  }
  if (result.error) {
    return { status: "unknown", message: result.error.message };
  }
  const combined = `${result.stdout}\n${result.stderr}`;
  return classifyAuthOutput(combined);
}
