import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";

import { Shimmer } from "@/components/ai-elements/shimmer";
import { MessageCopyButton } from "@/components/chat/MessageCopyButton";
import { MESSAGE_GROUP_CLASS } from "@/components/chat/message-action";
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

/** The active app theme and fonts, as handed to HTML renders. Stable until
 *  the theme or a font stack changes. */
function useHtmlRenderTheme(): HtmlRenderTheme {
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
  // The page is built with the theme it first mounts under; later changes
  // are posted to it, so switching themes never reloads the page or resets
  // its state.
  const [initialTheme] = useState(theme);
  const srcDoc = useMemo(() => buildHtmlRenderDocument(html, initialTheme), [html, initialTheme]);

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
      const url = readHtmlRenderLinkRequest(event.data);
      // Open only for a click the reader just made in this frame, so a page
      // cannot open links on its own.
      if (
        url !== undefined &&
        document.activeElement === frame &&
        navigator.userActivation?.isActive !== false
      ) {
        void openExternalUrl(url);
      }
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, []);

  const height = clampHtmlRenderHeight(
    Math.min(contentHeight ?? HTML_RENDER_INITIAL_HEIGHT, maxHeight ?? HTML_RENDER_MAX_HEIGHT),
  );

  return (
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
  );
}
