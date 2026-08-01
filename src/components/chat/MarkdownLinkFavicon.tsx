import { memo, useState } from "react";
import { Globe2 } from "lucide-react";

import { faviconUrlForDomain } from "@/lib/project-image";

const failedFaviconHosts = new Set<string>();

/**
 * Inline favicon inserted into external chat links by
 * `rehypeRichExternalLinks`. A session-wide failure cache avoids repeatedly
 * requesting a host that has already failed and falls back to a neutral globe.
 */
export const MarkdownLinkFavicon = memo(function MarkdownLinkFavicon({
  host,
}: Record<string, unknown>) {
  const hostname = typeof host === "string" ? host : "";
  const [failedHost, setFailedHost] = useState<string | null>(null);
  const failed = !hostname || failedHost === hostname || failedFaviconHosts.has(hostname);

  return (
    <span className="chat-markdown-link-favicon" aria-hidden="true">
      {failed ? (
        <Globe2 className="block size-full" />
      ) : (
        <img
          src={faviconUrlForDomain(hostname, 32)}
          alt=""
          loading="lazy"
          draggable={false}
          className="block size-full rounded-sm"
          onError={() => {
            failedFaviconHosts.add(hostname);
            setFailedHost(hostname);
          }}
        />
      )}
    </span>
  );
});
