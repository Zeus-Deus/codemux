/**
 * Human-readable rendering of the backend's `SerializableProviderError`.
 *
 * Provider commands (`agent_chat_start_session`, `agent_chat_send_turn`,
 * …) reject with a JSON-encoded `SerializableProviderError` string
 * (`{"kind":"not_installed","provider":"claude","hint":"…"}`). Toasting
 * that raw JSON at the user was the old behavior; this module parses it
 * back into a readable sentence, falling back to the raw string for
 * anything unrecognized (feature-flag errors, plain-string rejections).
 */

import type { AgentChatProviderKind } from "@/tauri/types";

const PROVIDER_LABEL: Record<AgentChatProviderKind, string> = {
  claude: "Claude",
  codex: "Codex",
  cursor: "Cursor",
  grok: "Grok",
  hermes: "Hermes",
  opencode: "OpenCode",
};

const GROK_MODEL_RESTART_PREFIX = "grok_model_restart_required:";

interface WireProviderError {
  kind?: unknown;
  provider?: unknown;
  hint?: unknown;
  message?: unknown;
  source?: unknown;
  operation?: unknown;
  elapsed_ms?: unknown;
  pane_id?: unknown;
  thread_id?: unknown;
}

/** A start-session claim the backend refused because another client got
 *  to the pane first. Carries the binding that won, so the losing client
 *  can adopt it instead of starting a rival session. */
export interface PaneAlreadyBound {
  paneId: string;
  threadId: string;
  provider: AgentChatProviderKind | null;
}

function labelOf(provider: unknown): string {
  if (typeof provider === "string" && provider in PROVIDER_LABEL) {
    return PROVIDER_LABEL[provider as AgentChatProviderKind];
  }
  return "Provider";
}

function str(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}

function errorText(raw: unknown): string {
  return typeof raw === "string"
    ? raw
    : raw instanceof Error
      ? raw.message
      : String(raw);
}

/**
 * Recognise the backend's `pane_already_bound` rejection.
 *
 * Every client shares one backend and one app-state snapshot, so two of
 * them can mount the same freshly created pane and both try to start a
 * session on it. The backend claims the pane before spawning anything and
 * rejects the loser with
 * `{"kind":"pane_already_bound","pane_id":…,"thread_id":…,"provider":…}`.
 * That is not a failure the user needs to see: the winning binding is in
 * the error, and the loser simply adopts it. Returns null for anything
 * else (including unparseable input), so callers fall through to their
 * normal error handling.
 */
export function parsePaneAlreadyBound(raw: unknown): PaneAlreadyBound | null {
  let wire: WireProviderError;
  try {
    const parsed: unknown = JSON.parse(errorText(raw));
    if (!parsed || typeof parsed !== "object") return null;
    wire = parsed as WireProviderError;
  } catch {
    return null;
  }
  if (wire.kind !== "pane_already_bound") return null;
  const paneId = str(wire.pane_id);
  const threadId = str(wire.thread_id);
  // A claim rejection without the winning binding tells us nothing to
  // adopt — let it surface as an ordinary error instead.
  if (!paneId || !threadId) return null;
  const provider = str(wire.provider);
  return {
    paneId,
    threadId,
    provider:
      provider !== null && provider in PROVIDER_LABEL
        ? (provider as AgentChatProviderKind)
        : null,
  };
}

/** Prefix the backend uses when a pane claim targets a pane that no longer
 *  exists (`pane_not_found: <pane id>`). */
const PANE_NOT_FOUND_PREFIX = "pane_not_found:";

/** Whether Grok rejected a live model switch because the selected model uses
 * a different agent family and therefore needs a fresh ACP session. */
export function grokModelChangeRequiresRestart(raw: unknown): boolean {
  const text = errorText(raw);
  if (text.includes(GROK_MODEL_RESTART_PREFIX)) return true;
  try {
    const parsed: unknown = JSON.parse(text);
    return (
      !!parsed &&
      typeof parsed === "object" &&
      str((parsed as WireProviderError).message)?.includes(
        GROK_MODEL_RESTART_PREFIX,
      ) === true
    );
  } catch {
    return false;
  }
}

/**
 * Best-effort: turn a rejected provider-command error into a sentence.
 * Never throws; unparseable input comes back verbatim (trimmed to a
 * string) so no information is lost.
 */
export function formatProviderError(raw: unknown): string {
  const text = errorText(raw);
  // `pane_not_found: <id>` is a bare string, not a wire error: the pane
  // claim ran against a pane that had been closed. Nothing in the raw form
  // means anything to the user, so translate it before the JSON parse.
  if (text.startsWith(PANE_NOT_FOUND_PREFIX)) {
    return "This chat pane was closed before the session could start.";
  }
  let wire: WireProviderError;
  try {
    const parsed: unknown = JSON.parse(text);
    if (!parsed || typeof parsed !== "object") return text;
    wire = parsed as WireProviderError;
  } catch {
    return text;
  }
  switch (wire.kind) {
    case "not_installed":
      return `${labelOf(wire.provider)} CLI is not installed. ${str(wire.hint) ?? ""}`.trim();
    case "not_authenticated":
      return `${labelOf(wire.provider)} CLI is not authenticated. ${str(wire.hint) ?? ""}`.trim();
    case "pane_already_bound":
      return "This chat pane is already running a session started elsewhere.";
    case "session_not_found":
      return "The chat session is no longer live. Try sending again to restart it.";
    case "session_closed":
      return "The chat session has been closed. Try sending again to restart it.";
    case "validation_error": {
      const message = str(wire.message) ?? text;
      return message.startsWith(GROK_MODEL_RESTART_PREFIX)
        ? message.slice(GROK_MODEL_RESTART_PREFIX.length).trim()
        : message;
    }
    case "process_error": {
      const message = str(wire.message) ?? "The provider process failed.";
      const source = str(wire.source);
      return source ? `${message} (${source})` : message;
    }
    case "rpc_error":
      return str(wire.message) ?? text;
    case "timeout": {
      const operation = str(wire.operation) ?? "The provider operation";
      const elapsed =
        typeof wire.elapsed_ms === "number"
          ? ` after ${Math.round(wire.elapsed_ms / 1000)}s`
          : "";
      return `${operation} timed out${elapsed}.`;
    }
    default:
      return text;
  }
}
