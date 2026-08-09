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
 */
export function ChatMarkdownLink({
  children,
  className,
  href,
  node: _node,
  onClick,
  ...props
}: ChatMarkdownLinkProps) {
  const [confirmationOpen, setConfirmationOpen] = useState(false);
  const external = externalWebLinkHost(href) !== null;
  const incomplete = href === INCOMPLETE_LINK;

  const link = (
    <a
      {...props}
      href={incomplete ? undefined : href}
      rel={external ? "noopener noreferrer" : props.rel}
      target={external ? "_blank" : props.target}
      data-incomplete={incomplete || undefined}
      data-streamdown="link"
      aria-disabled={incomplete || undefined}
      className={cn(
        "wrap-anywhere cursor-pointer font-medium text-primary underline",
        incomplete && "pointer-events-none opacity-50",
        className,
      )}
      onClick={(event) => {
        onClick?.(event);
        if (event.defaultPrevented || !external) return;
        event.preventDefault();
        setConfirmationOpen(true);
      }}
    >
      {children}
    </a>
  );

  if (!external || !href) return link;

  const openConfirmedLink = () => {
    void openUrl(href).catch((error) => {
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
          {href}
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
