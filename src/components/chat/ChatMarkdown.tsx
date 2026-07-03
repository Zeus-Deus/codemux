import remarkGfm from "remark-gfm";
import { Streamdown } from "streamdown";

import { cn } from "@/lib/utils";

/**
 * Shared markdown renderer for chat surfaces (assistant messages,
 * plan proposals).
 *
 * Streaming renderer: `streamdown` (the drop-in react-markdown
 * replacement shadcn's AI Elements are built on). `parseIncompleteMarkdown`
 * closes unterminated fences / emphasis / lists mid-stream so a
 * half-arrived token never flashes broken markup. We keep the same
 * `remarkGfm`-only plugin set the previous react-markdown renderer used
 * (no math/KaTeX) and skip streamdown's shiki `code` plugin, so fenced
 * blocks render as plain `<pre>` styled by the prose chain below — no
 * async highlighter, identical DOM semantics.
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
  // Inline code — typography plugin wraps inline code in backticks
  // by default (ugh). Override to subtle muted pill, strip the
  // quotes via the ::before/::after reset.
  "prose-code:font-mono prose-code:text-[0.85em] prose-code:bg-muted prose-code:text-foreground prose-code:rounded prose-code:px-1.5 prose-code:py-0.5 prose-code:border prose-code:border-border/60 prose-code:before:content-none prose-code:after:content-none",
  // Fenced code blocks — bordered card, muted fill, tight line
  // height so ASCII diagrams stay visually continuous.
  "prose-pre:my-3 prose-pre:rounded-md prose-pre:border prose-pre:border-border prose-pre:bg-muted prose-pre:text-foreground prose-pre:p-3 prose-pre:text-[0.8em] prose-pre:leading-snug prose-pre:overflow-x-auto",
  // Tables — plugin-default spacing is loose; pull in for chat.
  "prose-table:my-3 prose-table:text-[0.9em]",
  "prose-th:px-3 prose-th:py-1.5 prose-th:bg-muted/60 prose-th:font-semibold prose-th:text-foreground prose-th:border prose-th:border-border/70",
  "prose-td:px-3 prose-td:py-1.5 prose-td:align-top prose-td:border prose-td:border-border/60",
  // Blockquote — neutral left rule, no italic accent.
  "prose-blockquote:not-italic prose-blockquote:font-normal prose-blockquote:border-l-2 prose-blockquote:pl-3 prose-blockquote:my-2",
  // Break long tokens (URLs, identifiers) rather than overflowing.
  "[&_*]:break-words",
].join(" ");

export function ChatMarkdown({ children }: { children: string }) {
  return (
    <div className={cn(proseClasses)}>
      <Streamdown
        parseIncompleteMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[]}
      >
        {children}
      </Streamdown>
    </div>
  );
}
