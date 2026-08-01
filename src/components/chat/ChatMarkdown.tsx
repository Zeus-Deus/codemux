import { useMemo } from "react";
import remarkGfm from "remark-gfm";
import { Streamdown, type Components } from "streamdown";

import { useChatCodePlugin } from "@/hooks/use-chat-code-plugin";
import {
  CHAT_LINK_FAVICON_TAG,
  rehypeRichExternalLinks,
} from "@/lib/agent-chat/rich-links";
import { cn } from "@/lib/utils";
import { selectChatCodeWrap, useSettingsStore } from "@/stores/settings-store";
import { ChatMarkdownStreamingContext } from "./chat-markdown-streaming";
import { MarkdownLinkFavicon } from "./MarkdownLinkFavicon";

/**
 * Shared markdown renderer for chat surfaces (assistant messages,
 * plan proposals).
 *
 * Streaming renderer: `streamdown` (the drop-in react-markdown
 * replacement shadcn's AI Elements are built on). `parseIncompleteMarkdown`
 * closes unterminated fences / emphasis / lists mid-stream so a
 * half-arrived token never flashes broken markup. We keep the same
 * `remarkGfm`-only plugin set the previous react-markdown renderer used
 * (no math/KaTeX).
 *
 * Fenced code blocks are syntax-highlighted by streamdown's shiki `code`
 * plugin, themed from the terminal ANSI palette via `useChatCodePlugin`
 * so chat code matches the file editor and terminal panes.
 *
 * Note that streamdown renders its own code-block card (container, header,
 * copy/download pill, optional line numbers) whether or not the `code`
 * plugin is installed — it is not a bare `<pre>`. The prose chain below
 * therefore *neutralizes* the typography plugin's `pre`/`code` defaults
 * instead of styling them; the card itself is styled by design tokens in
 * `globals.css` under `.chat-markdown`. Styling `pre` here again would
 * stack a third border/background/padding inside that card.
 *
 * Built on `@tailwindcss/typography` (registered as a `@plugin` in
 * `globals.css`). The `prose` class provides a typography stack;
 * Tailwind's `prose-*` variants tune per-element styling in one
 * place here. Plugin defaults target long-form articles (blog-post
 * density) so the override chain below pulls margins tighter, maps
 * code / quotes / tables to our design tokens (`--muted`,
 * `--border`, `--foreground`), and forces list markers inside the
 * list's own padding so they never bleed through a parent card
 * border.
 *
 * To tweak the prose rhythm or colors, edit the class chain in
 * `proseClasses` — no CSS file edit needed.
 */

// Per-element override utilities. Each tunes one concern; the chain
// is flat on purpose so future tweaks touch exactly one group.
const proseClasses = [
  // Base prose stack, scaled down for chat density. `prose-invert`
  // under dark mode flips neutral tokens correctly via our shadcn
  // token setup.
  "prose prose-sm dark:prose-invert max-w-none",
  // Color tokens — map prose's neutral slots to shadcn tokens so
  // dark/light themes track the rest of the app.
  "prose-headings:text-foreground",
  "prose-p:text-foreground",
  "prose-li:text-foreground",
  "prose-strong:text-foreground",
  "prose-blockquote:text-muted-foreground prose-blockquote:border-border",
  "prose-hr:border-border",
  // Links — neutral underline, not the default "prose-invert red"
  // accent. Matches codemux-chat-ui skill.
  "prose-a:text-foreground prose-a:underline prose-a:underline-offset-2 prose-a:decoration-muted-foreground/60 hover:prose-a:decoration-foreground",
  // Rhythm — tighter margins than article defaults so plan bullets
  // don't feel like a doc page.
  "prose-p:my-2 prose-p:leading-relaxed",
  "prose-headings:mt-4 prose-headings:mb-2 prose-headings:leading-snug",
  "prose-h1:text-[1.05em] prose-h2:text-[1.05em] prose-h3:text-base prose-h4:text-base",
  "prose-ul:my-2 prose-ol:my-2",
  "prose-li:my-0.5 prose-li:leading-relaxed",
  // List markers — padding-left keeps markers inside the list box
  // (inside any parent card border even for `10.` / `100.` widths)
  // and muted-foreground tones them down.
  "prose-ul:pl-6 prose-ol:pl-6",
  "marker:text-muted-foreground",
  // Inline code — the typography plugin wraps inline code in literal
  // backticks and bolds it; strip both. The pill itself (muted fill,
  // border, padding) is streamdown's `inline-code` element, styled in
  // `globals.css` — don't restate it here or the two stack.
  "prose-code:before:content-none prose-code:after:content-none prose-code:font-normal",
  // Fenced code blocks — neutralize the typography plugin's `pre`
  // defaults (dark slab, padding, radius). Streamdown's card provides
  // the real surface; see `.chat-markdown` in `globals.css`.
  "prose-pre:m-0 prose-pre:p-0 prose-pre:bg-transparent prose-pre:text-inherit prose-pre:rounded-none prose-pre:border-0 prose-pre:leading-snug",
  // Tables — plugin-default spacing is loose; pull in for chat.
  "prose-table:my-3 prose-table:text-[0.9em]",
  "prose-th:px-3 prose-th:py-1.5 prose-th:bg-muted/60 prose-th:font-semibold prose-th:text-foreground prose-th:border prose-th:border-border/70",
  "prose-td:px-3 prose-td:py-1.5 prose-td:align-top prose-td:border prose-td:border-border/60",
  // Blockquote — neutral left rule, no italic accent.
  "prose-blockquote:not-italic prose-blockquote:font-normal prose-blockquote:border-l-2 prose-blockquote:pl-3 prose-blockquote:my-2",
  // Break long tokens (URLs, identifiers) rather than overflowing.
  "[&_*]:break-words",
].join(" ");

// Line numbers are an editor affordance; chat snippets are quotes, not
// files, and the gutter competes with the prose column. Tables keep their
// full control set — only the code block's chrome is trimmed.
const controls = {
  code: { copy: true, download: true },
  table: { copy: true, download: true, fullscreen: true },
} as const;

const markdownComponents: Components = {
  [CHAT_LINK_FAVICON_TAG]: MarkdownLinkFavicon,
};

const rehypePlugins = [rehypeRichExternalLinks];

/**
 * `streaming` marks markdown that is still arriving token by token. Only the
 * link favicons care today: a bare URL is autolinked on every frame as it
 * types out, so decorating an in-flight message would request icons for
 * hostname prefixes that never resolve (see `MarkdownLinkFavicon`).
 */
export function ChatMarkdown({
  children,
  streaming = false,
}: {
  children: string;
  streaming?: boolean;
}) {
  const code = useChatCodePlugin();
  const plugins = useMemo(() => ({ code }), [code]);
  const wrap = useSettingsStore(selectChatCodeWrap);

  return (
    <ChatMarkdownStreamingContext.Provider value={streaming}>
      <div className={cn("chat-markdown", proseClasses)} data-code-wrap={wrap}>
        <Streamdown
          parseIncompleteMarkdown
          remarkPlugins={[remarkGfm]}
          rehypePlugins={rehypePlugins}
          components={markdownComponents}
          plugins={plugins}
          controls={controls}
          lineNumbers={false}
        >
          {children}
        </Streamdown>
      </div>
    </ChatMarkdownStreamingContext.Provider>
  );
}
