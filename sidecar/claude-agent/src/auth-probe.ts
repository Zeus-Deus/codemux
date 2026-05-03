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

/** Classify combined stdout+stderr into an auth status. Split out
 *  so it is pure and unit-testable. */
export function classifyAuthOutput(output: string): ProbeAuthenticatedResult {
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
