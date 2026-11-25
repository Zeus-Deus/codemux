import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { ExternalLink } from "lucide-react";

import { Shimmer } from "@/components/ai-elements/shimmer";
import { MessageCopyButton } from "@/components/chat/MessageCopyButton";
import { MESSAGE_GROUP_CLASS } from "@/components/chat/message-action";
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
import {
  HTML_RENDER_INITIAL_HEIGHT,
  HTML_RENDER_MAX_HEIGHT,
  buildHtmlRenderDocument,
  clampHtmlRenderHeight,
  htmlRenderTheme,
  htmlRenderThemeMessage,
  readHtmlRender,
  readHtmlRenderContentHeight,
  readHtmlRenderLinkRequest,
  type HtmlRenderTheme,
} from "@/lib/agent-chat/html-render";
import type { ToolCallItem } from "@/lib/agent-chat/types";
import { openExternalUrl } from "@/lib/open-url";
import { getActiveTheme, subscribeActiveTheme } from "@/lib/themes";

// `applyTypography` writes the font stacks onto the root element's inline
// style, so a change there is the signal that they may have moved.
function subscribeRootStyle(listener: () => void): () => void {
  const observer = new MutationObserver(listener);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["style"] });
  return () => observer.disconnect();
}

function readFonts(): string {
  const root = getComputedStyle(document.documentElement);
  return [root.getPropertyValue("--font-interface"), root.getPropertyValue("--font-code")]
    .map((value) => value.trim())
    .join("\n");
}

function canonicalLinkRequest(value: string): string | null {
  // Keep a page from making the confirmation itself unwieldy. The parent
  // displays and opens the same browser-normalized address.
  if (value.length > 8192) return null;
  try {
    const url = new URL(value);
    if ((url.protocol !== "http:" && url.protocol !== "https:") || url.username || url.password) {
      return null;
    }
    return url.href;
  } catch {
    return null;
  }
}

/** The active app theme and fonts, as handed to HTML renders. Stable until
 *  the theme or a font stack changes. */
export function useHtmlRenderTheme(): HtmlRenderTheme {
  const theme = useSyncExternalStore(subscribeActiveTheme, getActiveTheme);
  const fonts = useSyncExternalStore(subscribeRootStyle, readFonts);
  return useMemo(() => {
    const [sans, mono] = fonts.split("\n");
    return htmlRenderTheme(theme, {
      sans: sans || "system-ui, sans-serif",
      mono: mono || "ui-monospace, monospace",
    });
  }, [theme, fonts]);
}

/**
 * An agent's `html_render` page inline in the thread: borderless on the
 * thread's own background, fitted to the page's reported height. The page
 * runs in a sandboxed frame with an opaque origin, so it cannot reach the
 * app, its IPC, or its storage. While the call is still in flight the row
 * holds a one-line placeholder.
 */
export const HtmlRenderFrame = memo(function HtmlRenderFrame({ item }: { item: ToolCallItem }) {
  const render = useMemo(() => readHtmlRender(item), [item]);
  if (item.status === "running" || render === null) {
    return (
      <div className="py-1 text-label text-muted-foreground">
        <Shimmer duration={1.5}>Building visualization…</Shimmer>
      </div>
    );
  }
  return (
    <div className={MESSAGE_GROUP_CLASS} data-testid="html-render">
      <HtmlRenderDocument title={render.title} html={render.html} maxHeight={render.maxHeight} />
      <MessageCopyButton text={render.html} label="Copy HTML" className="mt-1" />
    </div>
  );
});

function HtmlRenderDocument({
  title,
  html,
  maxHeight,
}: {
  title: string;
  html: string;
  maxHeight: number | null;
}) {
  const theme = useHtmlRenderTheme();
  const frameRef = useRef<HTMLIFrameElement>(null);
  const [loaded, setLoaded] = useState(false);
  const [contentHeight, setContentHeight] = useState<number>();
  const [pendingUrl, setPendingUrl] = useState<string | null>(null);
  const pendingUrlRef = useRef<string | null>(null);
  // The page is built with the theme it first mounts under; later changes
  // are posted to it, so switching themes never reloads the page or resets
  // its state.
  const [initialTheme] = useState(theme);
  const srcDoc = useMemo(() => buildHtmlRenderDocument(html, initialTheme), [html, initialTheme]);

  const dismissLink = () => {
    pendingUrlRef.current = null;
    setPendingUrl(null);
  };

  // A new page must not inherit an open-link request from the previous one.
  useLayoutEffect(dismissLink, [srcDoc]);

  const postTheme = () => {
    frameRef.current?.contentWindow?.postMessage(htmlRenderThemeMessage(theme), "*");
  };
  useEffect(postTheme, [theme]);

  // A page posts its height once per change, so listen from the commit that
  // inserts the frame; a passive effect could miss a fast page's first post.
  useLayoutEffect(() => {
    const onMessage = (event: MessageEvent) => {
      const frame = frameRef.current;
      if (!frame || event.source !== frame.contentWindow) return;
      const height = readHtmlRenderContentHeight(event.data);
      if (height !== undefined) {
        setContentHeight(height);
        return;
      }
      const requestedUrl = readHtmlRenderLinkRequest(event.data);
      const url = requestedUrl === undefined ? null : canonicalLinkRequest(requestedUrl);
      // Frame scripts can forge open-link messages, even after an unrelated
      // click. The parent owns the confirmation and keeps the first request
      // until the reader chooses what to do with it.
      if (
        url !== null &&
        pendingUrlRef.current === null &&
        document.activeElement === frame &&
        navigator.userActivation?.isActive !== false
      ) {
        pendingUrlRef.current = url;
        setPendingUrl(url);
      }
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, []);

  const height = clampHtmlRenderHeight(
    Math.min(contentHeight ?? HTML_RENDER_INITIAL_HEIGHT, maxHeight ?? HTML_RENDER_MAX_HEIGHT),
  );

  return (
    <>
      <iframe
        ref={frameRef}
        srcDoc={srcDoc}
        title={title}
        // Never allow-same-origin: the opaque origin keeps the page out of the app.
        sandbox="allow-scripts"
        className="block w-full border-0"
        // A frame whose color scheme differs from its document's paints an
        // opaque canvas, so the blank document a frame starts with would flash
        // white on a dark theme. Once the page is in, the frame follows it.
        style={{ height, colorScheme: loaded ? theme.scheme : "light" }}
        onLoad={() => {
          setLoaded(true);
          // Covers a theme change that landed while the page was loading.
          postTheme();
        }}
      />
      <AlertDialog open={pendingUrl !== null} onOpenChange={(open) => !open && dismissLink()}>
        <AlertDialogContent className="flex max-h-[calc(100dvh-2rem)] flex-col gap-2 overflow-hidden">
          <AlertDialogHeader className="min-h-0 overflow-y-auto [@media(max-height:320px)]:overflow-hidden">
            <AlertDialogMedia className="[@media(max-height:320px)]:hidden">
              <ExternalLink aria-hidden="true" />
            </AlertDialogMedia>
            <AlertDialogTitle>Open external link?</AlertDialogTitle>
            <AlertDialogDescription className="[@media(max-height:320px)]:hidden">
              This page asked to open a link. Check the address before continuing.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <div className="shrink-0 rounded-lg bg-muted px-3 py-1.5 text-label">
            <div className="text-muted-foreground">Destination origin</div>
            <bdi
              dir="ltr"
              role="region"
              aria-label="Destination origin"
              tabIndex={0}
              className="block max-h-12 select-text overflow-y-auto break-all font-mono text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-focus-ring [unicode-bidi:isolate]"
            >
              {pendingUrl === null ? null : new URL(pendingUrl).origin}
            </bdi>
          </div>
          <div
            role="region"
            aria-label="Full destination address"
            tabIndex={0}
            className="min-h-12 basis-0 flex-1 overflow-y-auto rounded-lg bg-muted px-3 py-2 text-label focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-focus-ring"
          >
            <bdi dir="ltr" className="mt-1 block select-text break-all font-mono text-muted-foreground [unicode-bidi:isolate]">
              {pendingUrl}
            </bdi>
          </div>
          <AlertDialogFooter className="shrink-0 py-2">
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (pendingUrl !== null) void openExternalUrl(pendingUrl);
                dismissLink();
              }}
            >
              <ExternalLink data-icon="inline-start" aria-hidden="true" />
              Open link
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
