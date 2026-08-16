/**
 * Recognising a pull-request URL, and finding the row it names.
 *
 * A host pull-request URL is the one link in Codemux that has somewhere
 * better to go than the browser: the app already has a page that renders
 * that pull request, with the diff, the checks and the review bar on it.
 * So the URL is parsed here, and — only if the repository it names is
 * actually open — routed inward instead of outward.
 *
 * Two deliberate limits:
 *
 * - **Known hosts only.** A URL is classified by its hostname, from the
 *   products' canonical domains plus whatever the user has declared in
 *   Settings ▸ Source control. Guessing a self-hosted instance from its
 *   path shape would let this hijack links it has no business opening.
 * - **Open repositories only.** Routing inward is a promise that the
 *   page can show the thing. If no open project has that pull request,
 *   the link keeps its original destination and says why.
 *
 * Pure parsing, no side effects — see `open-url.ts` for the routing.
 */

import type { PrRow } from "@/lib/pr-overview";
import { GITHUB_HOSTS, GITLAB_HOSTS, type ProviderKind } from "@/lib/source-control";

export interface ParsedPrUrl {
  kind: Extract<ProviderKind, "github" | "gitlab">;
  host: string;
  /** `owner/name` — the last two path segments before the change-request
   *  segment, so GitLab subgroups resolve to the project. */
  slug: string;
  number: number;
}

/** Which product serves this host, from the canonical domains plus the
 *  user's declared self-hosted instances. */
export function hostKind(
  host: string,
  customHosts: Record<string, string> | undefined,
): ProviderKind | null {
  const bare = host.trim().toLowerCase().replace(/^www\./, "");
  if (GITHUB_HOSTS.includes(bare)) return "github";
  if (GITLAB_HOSTS.includes(bare)) return "gitlab";
  const declared = customHosts?.[bare] ?? customHosts?.[host.trim().toLowerCase()];
  if (declared === "github" || declared === "gitlab") return declared;
  return null;
}

const GITHUB_PATH = /^\/(.+?)\/(.+?)\/pull\/(\d+)(?:[/?#].*)?$/;
const GITLAB_PATH = /^\/(.+?)\/-\/merge_requests\/(\d+)(?:[/?#].*)?$/;

/**
 * The pull request a URL names, or null when it names something else.
 *
 * Trailing segments (`/files`, `/commits`) and fragments are tolerated:
 * a link to a comment on a pull request is still a link to that pull
 * request, and the page opens at the top of it either way.
 */
export function parsePrUrl(
  url: string,
  customHosts?: Record<string, string>,
): ParsedPrUrl | null {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return null;
  }
  if (parsed.protocol !== "https:" && parsed.protocol !== "http:") return null;

  const kind = hostKind(parsed.hostname, customHosts);
  if (kind !== "github" && kind !== "gitlab") return null;

  const path = parsed.pathname.replace(/\/+$/, "") + (parsed.search || parsed.hash ? "" : "");
  const full = `${path}${parsed.search}${parsed.hash}`;

  if (kind === "github") {
    const match = GITHUB_PATH.exec(full);
    if (!match) return null;
    const number = Number(match[3]);
    if (!Number.isInteger(number) || number <= 0) return null;
    return { kind, host: parsed.hostname.toLowerCase(), slug: `${match[1]}/${match[2]}`, number };
  }

  const match = GITLAB_PATH.exec(full);
  if (!match) return null;
  const number = Number(match[2]);
  if (!Number.isInteger(number) || number <= 0) return null;
  const segments = match[1].split("/").filter(Boolean);
  if (segments.length < 2) return null;
  return {
    kind,
    host: parsed.hostname.toLowerCase(),
    slug: segments.slice(-2).join("/"),
    number,
  };
}

// ── The index the router looks a parsed URL up in ─────────────────────
//
// The overview query is a React hook and the link handler is a plain
// function called from a click, so the app-level watcher publishes its
// rows here rather than every caller having to be a component.

let indexedRows: PrRow[] = [];

/** Called by the app-level pull-request watcher on every poll. */
export function publishPrLinkIndex(rows: PrRow[]): void {
  indexedRows = rows;
}

/** Test seam. */
export function _resetPrLinkIndex(): void {
  indexedRows = [];
}

/**
 * The open row this URL names, if any.
 *
 * Matched on repository slug *and* number: a number alone is only unique
 * within one repository, and two open projects can easily both have a
 * `#12`.
 */
export function resolvePrLink(parsed: ParsedPrUrl, rows: PrRow[] = indexedRows): PrRow | null {
  const slug = parsed.slug.toLowerCase();
  return (
    rows.find(
      (row) => row.number === parsed.number && (row.repo ?? "").toLowerCase() === slug,
    ) ?? null
  );
}
