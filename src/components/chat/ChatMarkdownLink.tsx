import { useState, type ComponentProps } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ExternalLink } from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogMedia,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { externalWebLinkHost } from "@/lib/agent-chat/rich-links";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";

const INCOMPLETE_LINK = "streamdown:incomplete-link";

type ChatMarkdownLinkProps = ComponentProps<"a"> & {
  node?: unknown;
};

/**
 * Chat transcript link with a portal-backed external-link confirmation.
 *
 * Streamdown's built-in safety modal is mounted beside the link. LegendList
 * rows use paint/layout containment, so that fixed modal is clipped to the
 * virtualized row and can leave only its dimming backdrop visible. The shared
 * AlertDialog primitive portals to document.body and therefore stays in the
 * viewport stacking context.
 *
 * The webview never performs a link's navigation itself. Chat markdown is
 * agent-relayed text, and `ChatMarkdown` passes its own `rehypePlugins`, which
 * REPLACES Streamdown's sanitize/harden chain — so an href reaches this
 * component exactly as written. Only an absolute `http(s)` URL is treated as a
 * destination, and only through the confirmation dialog plus Tauri's system
 * browser opener. Every other href (script schemes, `file:`, `mailto:`,
 * absolute paths, relative paths, fragments) renders without a live `href`:
 * following one in place would either execute in the app origin with IPC reach
 * or unload the single-page app.
 */
export function ChatMarkdownLink({
  children,
  className,
  href,
  node: _node,
  onAuxClick,
  onClick,
  rel,
  target: _target,
  title,
  ...props
}: ChatMarkdownLinkProps) {
  const [confirmationOpen, setConfirmationOpen] = useState(false);
  const incomplete = href === INCOMPLETE_LINK;
  const externalHref =
    !incomplete && typeof href === "string" && externalWebLinkHost(href) !== null
      ? href
      : null;
  // A destination we refuse to navigate to, but still worth surfacing on hover
  // so the label is not the only clue about where the agent pointed.
  const inertHref = !incomplete && !externalHref ? href : undefined;

  const link = (
    <a
      {...props}
      href={externalHref ?? undefined}
      rel={externalHref ? "noopener noreferrer" : rel}
      title={title ?? inertHref}
      data-incomplete={incomplete || undefined}
      data-inert-link={inertHref ? "" : undefined}
      data-streamdown="link"
      aria-disabled={incomplete || undefined}
      className={cn(
        "wrap-anywhere font-medium text-primary underline",
        externalHref ? "cursor-pointer" : "cursor-default",
        incomplete && "pointer-events-none opacity-50",
        className,
      )}
      onClick={(event) => {
        onClick?.(event);
        const handledByCaller = event.defaultPrevented;
        event.preventDefault();
        if (handledByCaller || !externalHref) return;
        setConfirmationOpen(true);
      }}
      onAuxClick={(event) => {
        onAuxClick?.(event);
        // Middle click never fires `onClick`, so a live href would request a
        // background navigation that skipped the confirmation entirely.
        event.preventDefault();
      }}
    >
      {children}
    </a>
  );

  if (!externalHref) return link;

  const openConfirmedLink = () => {
    void openUrl(externalHref).catch((error) => {
      toast.error("Could not open the link", {
        description: error instanceof Error ? error.message : String(error),
      });
    });
  };

  return (
    <AlertDialog open={confirmationOpen} onOpenChange={setConfirmationOpen}>
      {link}
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogMedia>
            <ExternalLink aria-hidden="true" />
          </AlertDialogMedia>
          <AlertDialogTitle>Open external link?</AlertDialogTitle>
          <AlertDialogDescription>
            This address will open in your system browser.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <div className="select-text break-all rounded-lg bg-muted px-3 py-2 font-mono text-xs text-muted-foreground">
          {externalHref}
        </div>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction onClick={openConfirmedLink}>
            <ExternalLink data-icon="inline-start" aria-hidden="true" />
            Open link
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
