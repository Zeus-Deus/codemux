/**
 * The browser's navigation verbs, in one place.
 *
 * Two surfaces drive the same Chromium — the main-area pane's
 * {@link BrowserToolbar} and the right-panel deck's browser pane bar — and
 * both go through the agent-browser command channel rather than the
 * `browser_history_back`/`browser_reload` Tauri commands, so a click and an
 * agent's `codemux browser back` take the identical path.
 *
 * `sessionId` is always the session's `cli_session_name` where one exists
 * (see `effectiveSessionId` in `BrowserPane`), never the raw `browser-N` id.
 */
import { agentBrowserRun } from "@/tauri/commands";

export type BrowserNavAction = "back" | "forward" | "reload";

export function runBrowserNav(
  sessionId: string,
  action: BrowserNavAction,
): Promise<unknown> {
  return agentBrowserRun(sessionId, action, {});
}

/** What the user typed, as something the browser can actually open. */
export function normalizeBrowserUrl(url: string): string {
  return url.includes("://") || url.startsWith("data:") || url.startsWith("about:")
    ? url
    : `https://${url}`;
}

/**
 * The URL as a breadcrumb: scheme and `www.` dropped, trailing slash gone.
 *
 * The deck's pane bar gives the crumb a single truncating line, so the
 * host has to survive; `https://` in front of it is 8 characters of noise
 * that would push the host out first on a narrow panel.
 */
export function browserUrlCrumb(url: string | null | undefined): string {
  if (!url || url === "about:blank") return "about:blank";
  const stripped = url.replace(/^[a-z]+:\/\//i, "").replace(/^www\./i, "");
  const trimmed = stripped.replace(/\/$/, "");
  return trimmed || url;
}
