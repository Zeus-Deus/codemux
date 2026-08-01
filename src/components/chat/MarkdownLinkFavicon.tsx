import { memo, useState } from "react";
import { Globe2 } from "lucide-react";

import { isPublicWebHost } from "@/lib/agent-chat/rich-links";
import { faviconUrlForDomain } from "@/lib/project-image";
import { useChatMarkdownStreaming } from "./chat-markdown-streaming";

/** How long a host stays blacklisted after a failed favicon request. */
const FAILURE_TTL_MS = 5 * 60 * 1000;
/** Upper bound on remembered failures so a long session can't grow unbounded. */
const FAILURE_CACHE_LIMIT = 200;

/** host -> timestamp of the failed request. */
const failedFaviconHosts = new Map<string, number>();

function hasRecentlyFailed(host: string): boolean {
  const failedAt = failedFaviconHosts.get(host);
  if (failedAt === undefined) return false;
  if (Date.now() - failedAt < FAILURE_TTL_MS) return true;
  failedFaviconHosts.delete(host);
  return false;
}

function rememberFailure(host: string) {
  failedFaviconHosts.delete(host);
  failedFaviconHosts.set(host, Date.now());
  // Map iterates in insertion order, so the oldest entries drop out first.
  while (failedFaviconHosts.size > FAILURE_CACHE_LIMIT) {
    const oldest = failedFaviconHosts.keys().next().value;
    if (oldest === undefined) break;
    failedFaviconHosts.delete(oldest);
  }
}

/** Test seam — clears the module-level failure cache between cases. */
export function resetFaviconFailureCache() {
  failedFaviconHosts.clear();
}

/**
 * Inline favicon inserted into external chat links by
 * `rehypeRichExternalLinks`.
 *
 * Three cases render a neutral globe with no network request at all:
 *
 * - **Still streaming.** A bare URL is autolinked on every frame while it
 *   arrives, so `docs.p`, `docs.pyt`, … would each fire a request for a
 *   hostname that never existed — flicker plus junk in the failure cache.
 *   Real icons load once the message settles.
 * - **Non-public hosts.** Intranet names and private IP literals are never
 *   sent to the third-party favicon service (see `isPublicWebHost`).
 * - **Recent failures.** A short TTL stops a dead host being retried on every
 *   render without permanently downgrading it after one offline moment.
 */
export const MarkdownLinkFavicon = memo(function MarkdownLinkFavicon({
  host,
}: Record<string, unknown>) {
  const streaming = useChatMarkdownStreaming();
  const hostname = typeof host === "string" ? host : "";
  const [failedHost, setFailedHost] = useState<string | null>(null);
  const fallback =
    streaming ||
    !isPublicWebHost(hostname) ||
    failedHost === hostname ||
    hasRecentlyFailed(hostname);

  return (
    <span className="chat-markdown-link-favicon" aria-hidden="true">
      {fallback ? (
        <Globe2 className="block size-full" />
      ) : (
        <img
          src={faviconUrlForDomain(hostname, 32)}
          alt=""
          loading="lazy"
          draggable={false}
          className="block size-full rounded-sm"
          onError={() => {
            rememberFailure(hostname);
            setFailedHost(hostname);
          }}
        />
      )}
    </span>
  );
});
