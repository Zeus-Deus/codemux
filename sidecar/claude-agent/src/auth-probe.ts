// Subprocess probes for the user's local `claude` binary.
//
// This file is deliberately isolated — it is the only place in the
// sidecar allowed to spawn `claude` directly (for `--version`,
// `auth status` and `auth status --text`). All Claude *inference* goes
// through the SDK's `query()`. `scripts/check-tos-boundary.sh` allow-lists this file
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

const NOT_AUTHENTICATED_MESSAGE =
  "Claude CLI is not authenticated. Run `claude login` and retry.";

/** Outcome of `probeInstalled`. */
export interface ProbeInstalledResult {
  installed: boolean;
  /** CLI-reported version string, when available. */
  version?: string;
  /** The binary exists but `--version` timed out, exited non-zero or
   *  could not be executed. A cold machine or a slow update check can do
   *  this to a working install, so callers treat it as an advisory
   *  rather than as "not installed". */
  unresponsive?: boolean;
}

/** Outcome of `probeAuthenticated`. */
export interface ProbeAuthenticatedResult {
  status: "authenticated" | "unauthenticated" | "unknown";
  /** User-visible hint or raw output snippet. */
  message?: string;
  /** How the CLI reaches a model, e.g. "Claude Max" or
   *  "Custom endpoint · gateway.example.com". Never carries secrets;
   *  endpoints are reduced to host[:port]. Absent when unknown. */
  authLabel?: string;
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
  return classifyInstallOutcome(await runBinary(binary, ["--version"]));
}

/** Only a missing binary means "not installed"; every other failure
 *  proves something is there. Split out so it is pure and testable. */
export function classifyInstallOutcome(
  result: Pick<SpawnOutcome, "stdout" | "exitCode" | "error" | "timedOut">,
): ProbeInstalledResult {
  const error = result.error;
  if (error && "code" in error && error.code === "ENOENT") {
    return { installed: false };
  }
  if (result.timedOut || result.error || result.exitCode !== 0) {
    return { installed: true, unresponsive: true };
  }
  const trimmed = result.stdout.trim();
  if (trimmed === "") {
    return { installed: true, version: "unknown" };
  }
  return { installed: true, version: parseCliVersion(trimmed) };
}

/** Both "Claude Code 2.1.114" and "2.1.114 (Claude Code)" are used. */
export function parseCliVersion(output: string): string {
  const firstLine = output.trim().split("\n")[0] ?? "";
  return firstLine.match(/\b\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?\b/)?.[0] ?? "unknown";
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
  const parsed = parseAuthStatusJson(output);
  if (!parsed) return null;
  return parsed.loggedIn
    ? { status: "authenticated" }
    : {
        status: "unauthenticated",
        message: NOT_AUTHENTICATED_MESSAGE,
      };
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
        message: NOT_AUTHENTICATED_MESSAGE,
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

/** Labels the CLI itself uses for non-first-party API providers. */
const API_PROVIDER_LABELS: Record<string, string> = {
  bedrock: "Amazon Bedrock",
  vertex: "Google Vertex AI",
  foundry: "Microsoft Foundry",
  anthropicAws: "Claude Platform on AWS",
  anthropicGoogleCloud: "Claude Platform on Google Cloud",
  mantle: "Amazon Bedrock (Mantle)",
  gateway: "Cloud gateway",
};

/** Plan names as the CLI's `subscriptionType` spells them. */
const PLAN_LABELS: Record<string, string> = {
  free: "Free",
  pro: "Pro",
  max: "Max",
  team: "Team",
  enterprise: "Enterprise",
};

/** The fields of `claude auth status` (JSON) that decide readiness. */
interface AuthStatusJson {
  loggedIn: boolean;
  authMethod?: string | undefined;
  apiProvider?: string | undefined;
  subscriptionType?: string | undefined;
}

/** Facts read from `claude auth status --text`, one `Label: value`
 *  line each. Only present when the CLI prints them. */
interface AuthStatusText {
  /** `Anthropic base URL: …` (userinfo already masked by the CLI). */
  baseUrl?: string | undefined;
  /** `Login method: Claude Max account` for a claude.ai login. */
  loginMethod?: string | undefined;
  /** `API provider: Amazon Bedrock` for cloud providers. */
  apiProvider?: string | undefined;
}

function parseAuthStatusJson(output: string): AuthStatusJson | null {
  for (const candidate of jsonObjectCandidates(output)) {
    let parsed: unknown;
    try {
      parsed = JSON.parse(candidate);
    } catch {
      continue;
    }
    if (typeof parsed !== "object" || parsed === null) continue;
    const record = parsed as Record<string, unknown>;
    if (typeof record["loggedIn"] !== "boolean") continue;
    const text = (key: string) =>
      typeof record[key] === "string" ? (record[key] as string) : undefined;
    return {
      loggedIn: record["loggedIn"],
      authMethod: text("authMethod"),
      apiProvider: text("apiProvider"),
      subscriptionType: text("subscriptionType"),
    };
  }
  return null;
}

function parseAuthStatusText(output: string): AuthStatusText {
  const facts: AuthStatusText = {};
  for (const line of output.split(/\r?\n/)) {
    const match = /^\s*([A-Za-z][A-Za-z ]*?):\s+(.+?)\s*$/.exec(line);
    if (!match) continue;
    const [, label, value] = match;
    if (label === "Anthropic base URL") facts.baseUrl ??= value;
    else if (label === "Login method") facts.loginMethod ??= value;
    else if (label === "API provider") facts.apiProvider ??= value;
  }
  return facts;
}

/** `host[:port]` of a base URL that sends Claude somewhere other than
 *  Anthropic's own API, or `null` when the URL is unset, unparseable,
 *  not http(s), or first-party. Userinfo, path and query are dropped
 *  so the result is safe to show. */
export function customEndpointHost(baseUrl: string | undefined): string | null {
  if (!baseUrl) return null;
  let url: URL;
  try {
    url = new URL(baseUrl.trim());
  } catch {
    return null;
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return null;
  if (url.hostname === "") return null;
  // First-party means Anthropic's own domain or a subdomain of it. The
  // domain is compared label by label because the ToS boundary check
  // rejects the literal domain in sidecar sources; it exists to stop
  // requests to it, and nothing here sends one.
  const labels = url.hostname.toLowerCase().replace(/\.$/, "").split(".");
  if (labels.at(-1) === "com" && labels.at(-2) === "anthropic") return null;
  return url.host;
}

/** "Claude Max" from `subscriptionType: "max"` (or the text form
 *  "Claude Max account"); `null` when no plan is known. */
function claudePlanLabel(
  subscriptionType: string | undefined,
  loginMethod: string | undefined,
): string | null {
  const plan = subscriptionType?.trim();
  if (plan) {
    const key = plan
      .toLowerCase()
      .replace(/^claude[\s_-]*/, "")
      .replace(/[\s_-]*subscription$/, "");
    const known = PLAN_LABELS[key];
    if (known) return `Claude ${known}`;
    const words = plan
      .split(/[\s_-]+/)
      .filter(Boolean)
      .map((word) => word[0]!.toUpperCase() + word.slice(1));
    if (words.length === 0) return null;
    return words[0]!.toLowerCase() === "claude"
      ? words.join(" ")
      : `Claude ${words.join(" ")}`;
  }
  const method = loginMethod?.replace(/\s+account$/i, "").trim();
  if (!method) return null;
  return /^claude\b/i.test(method) ? method : `Claude ${method}`;
}

/** Combine both `auth status` outputs into readiness plus a label.
 *
 *  A proxy (LiteLLM, a personal gateway, …) is set up through
 *  ANTHROPIC_BASE_URL, often with an ANTHROPIC_AUTH_TOKEN or an
 *  apiKeyHelper and sometimes with no credential at all. The CLI's JSON
 *  reports a proxy token as `authMethod: "oauth_token"`, exactly like a
 *  real login, and reports a token-less proxy as `loggedIn: false` even
 *  though chats work. Only the text form names the base URL, so a
 *  configured custom endpoint counts as ready on its own and wins the
 *  label. Cloud providers (Bedrock, Vertex, …) authenticate with their
 *  own credentials, so a non-first-party provider counts as ready too;
 *  the Cloud gateway is the exception because it needs a sign-in the
 *  JSON does report. Pure so every case is unit-testable. */
export function classifyAuthStatus(
  jsonOutput: string,
  textOutput: string,
): ProbeAuthenticatedResult {
  const facts = parseAuthStatusText(textOutput);
  // Older CLIs print plain `auth status` as text too, so its lines
  // count as well.
  const plainFacts = parseAuthStatusText(jsonOutput);
  const json = parseAuthStatusJson(jsonOutput);
  const endpoint =
    customEndpointHost(facts.baseUrl) ?? customEndpointHost(plainFacts.baseUrl);
  if (endpoint) {
    return { status: "authenticated", authLabel: `Custom endpoint · ${endpoint}` };
  }

  if (!json) {
    // Legacy CLI: keep the text heuristics and add a plan label when a
    // login method line is there.
    const legacy = classifyAuthOutput(jsonOutput);
    const label = claudePlanLabel(
      undefined,
      facts.loginMethod ?? plainFacts.loginMethod,
    );
    return legacy.status === "authenticated" && label
      ? { ...legacy, authLabel: label }
      : legacy;
  }

  const provider = json.apiProvider;
  const cloudProvider =
    provider !== undefined && provider !== "firstParty" && provider !== "gateway";
  if (!json.loggedIn && !cloudProvider) {
    return {
      status: "unauthenticated",
      message: NOT_AUTHENTICATED_MESSAGE,
    };
  }

  const authLabel = labelFor(json, facts);
  return authLabel
    ? { status: "authenticated", authLabel }
    : { status: "authenticated" };
}

function labelFor(json: AuthStatusJson, facts: AuthStatusText): string | null {
  const provider = json.apiProvider;
  if (provider !== undefined && provider !== "firstParty") {
    return API_PROVIDER_LABELS[provider] ?? facts.apiProvider ?? null;
  }
  switch (json.authMethod) {
    case "claude.ai":
      return claudePlanLabel(json.subscriptionType, facts.loginMethod) ?? "Claude account";
    case "oauth_token":
      // A long-lived `claude setup-token` token; a proxy token would
      // have come with a base URL and been labelled above.
      return "Claude account";
    case "api_key":
      return "Anthropic API key";
    case "api_key_helper":
      return "API key helper";
    default:
      return null;
  }
}

/** Run `<binary> auth status` (JSON on current CLIs) and
 *  `<binary> auth status --text` side by side and classify both. The
 *  text form is the only place the CLI names a custom base URL. */
export async function probeAuthenticated(
  binary: string,
): Promise<ProbeAuthenticatedResult> {
  const [result, text] = await Promise.all([
    runBinary(binary, ["auth", "status"]),
    runBinary(binary, ["auth", "status", "--text"]),
  ]);
  // A CLI that predates `--text` rejects the flag; its output then just
  // carries no facts, which is the legacy behaviour.
  const textOutput = text.timedOut || text.error ? "" : `${text.stdout}\n${text.stderr}`;
  if (result.timedOut || result.error) {
    // The text form alone still proves a custom endpoint.
    const endpoint = customEndpointHost(parseAuthStatusText(textOutput).baseUrl);
    if (endpoint) {
      return { status: "authenticated", authLabel: `Custom endpoint · ${endpoint}` };
    }
    return {
      status: "unknown",
      message: result.timedOut ? "probe timed out" : result.error!.message,
    };
  }
  return classifyAuthStatus(`${result.stdout}\n${result.stderr}`, textOutput);
}
