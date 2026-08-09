import type {
  CodeHighlighterPlugin,
  HighlightResult,
} from "@streamdown/code";
import {
  CheckIcon,
  CopyIcon,
  WrapTextIcon,
} from "lucide-react";
import {
  createContext,
  type CSSProperties,
  type ComponentProps,
  type ReactNode,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { BundledLanguage } from "shiki";
import type { Components, ExtraProps } from "streamdown";
import { useIsCodeFenceIncomplete } from "streamdown";

import {
  FileTypeIcon,
  hasSpecificFileTypeIcon,
} from "@/components/icons/file-type-icon";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { resolveChatFileLink } from "@/lib/agent-chat/file-links";

import { useChatFileLinkContext } from "./chat-file-link-context";
import { MarkdownFileLink } from "./MarkdownFileLink";

interface ChatCodeRendererContextValue {
  defaultWrap: boolean;
  highlighter: CodeHighlighterPlugin;
}

const ChatCodeRendererContext =
  createContext<ChatCodeRendererContextValue | null>(null);

export function ChatCodeRendererProvider({
  children,
  defaultWrap,
  highlighter,
}: ChatCodeRendererContextValue & { children: ReactNode }) {
  const value = useMemo(
    () => ({ defaultWrap, highlighter }),
    [defaultWrap, highlighter],
  );
  return (
    <ChatCodeRendererContext.Provider value={value}>
      {children}
    </ChatCodeRendererContext.Provider>
  );
}

function useChatCodeRenderer(): ChatCodeRendererContextValue {
  const value = useContext(ChatCodeRendererContext);
  if (!value) {
    throw new Error("Chat code blocks must be rendered inside ChatCodeRendererProvider");
  }
  return value;
}

const CODE_FENCE_LANGUAGE_REGEX = /(?:^|\s)language-([^\s]+)/;
const FENCE_TITLE_ATTR_REGEX =
  /(?:^|\s)(?:title|file(?:name)?)=(?:"([^"]+)"|'([^']+)'|(\S+))/i;
// A bare fence token is only treated as a filename when it ends in something
// that actually looks like a file extension — alphabetic first character, so
// ```txt 1.5 and ```js v2.0 keep rendering as plain language fences instead of
// captioning the block "1.5" / "v2.0". A leading dot is allowed for dotfiles
// (`.env.local`).
const FENCE_FILENAME_TOKEN_REGEX = /^[\w@.][\w@./-]*\.[A-Za-z][A-Za-z0-9]*$/;
// ...and only when what precedes that extension isn't itself a version number:
// `2.0.x` clears the extension test, and `2.tsx` is far likelier to be a
// version or a line range than a path. Tested against the basename alone, so
// real files that merely start with a version (`v2.config.js`) still caption.
const VERSION_LIKE_BASENAME_REGEX = /^v?\d+(?:\.\d+)*$/i;

function isFenceFilenameToken(candidate: string): boolean {
  if (!FENCE_FILENAME_TOKEN_REGEX.test(candidate)) return false;
  const basename = candidate.slice(0, candidate.lastIndexOf("."));
  return !VERSION_LIKE_BASENAME_REGEX.test(basename);
}

const LANGUAGE_EXTENSION_ALIASES: Record<string, string> = {
  bash: "sh",
  "c#": "cs",
  "c++": "cpp",
  console: "sh",
  csharp: "cs",
  javascript: "js",
  markdown: "md",
  plaintext: "txt",
  python: "py",
  react: "tsx",
  ruby: "rb",
  rust: "rs",
  shell: "sh",
  shellscript: "sh",
  typescript: "ts",
  yaml: "yml",
};

export function extractFenceLanguage(className: string | undefined): string {
  return className?.match(CODE_FENCE_LANGUAGE_REGEX)?.[1]?.toLowerCase() ?? "text";
}

export function extractFenceTitle(meta: string | undefined): string | null {
  if (!meta) return null;
  const attributeMatch = FENCE_TITLE_ATTR_REGEX.exec(meta);
  const attributeTitle =
    attributeMatch?.[1] ?? attributeMatch?.[2] ?? attributeMatch?.[3];
  if (attributeTitle) return attributeTitle;
  return meta.split(/\s+/).find(isFenceFilenameToken) ?? null;
}

export function syntheticFilenameForLanguage(language: string): string {
  const normalized = language.toLowerCase();
  if (normalized === "dockerfile") return "Dockerfile";
  return `file.${LANGUAGE_EXTENSION_ALIASES[normalized] ?? normalized}`;
}

function nodeToPlainText(node: ReactNode): string {
  if (typeof node === "string" || typeof node === "number") {
    return String(node);
  }
  if (Array.isArray(node)) {
    return node.map(nodeToPlainText).join("");
  }
  if (node && typeof node === "object" && "props" in node) {
    return nodeToPlainText(
      (node as { props: { children?: ReactNode } }).props.children,
    );
  }
  return "";
}

type HighlightedToken = HighlightResult["tokens"][number][number];

function plainToken(content: string): HighlightedToken {
  return {
    content,
    color: "inherit",
    bgColor: "transparent",
    htmlStyle: {},
    offset: 0,
  };
}

function rawHighlightResult(code: string): HighlightResult {
  return {
    bg: "transparent",
    fg: "inherit",
    tokens: code.split("\n").map((line) => [plainToken(line)]),
  };
}

/**
 * Previously highlighted tokens plus the tail Shiki has not tokenized yet,
 * carried uncolored so streamed text is on screen the frame it arrives.
 */
function withPlainTail(result: HighlightResult, tail: string): HighlightResult {
  if (!tail) return result;
  const [firstLine, ...restLines] = tail.split("\n");
  const tokens = result.tokens.map((line) => [...line]);
  if (firstLine) {
    const lastLine = tokens[tokens.length - 1];
    if (lastLine) lastLine.push(plainToken(firstLine));
    else tokens.push([plainToken(firstLine)]);
  }
  for (const line of restLines) tokens.push([plainToken(line)]);
  return { ...result, tokens };
}

type HighlightedEntry = {
  cacheKey: string;
  code: string;
  language: string;
  result: HighlightResult;
};

/**
 * Highlighted tokens for one fence, stale-while-revalidating.
 *
 * `highlight()` only answers synchronously on a cache hit, and a streaming
 * fence changes on every token, so reading "the result for exactly this code,
 * else raw" drops the whole block back to uncolored text between every append
 * and re-colors it a tick later — a full-block flicker lasting the entire
 * stream. Instead the last result stays on screen while the next one is
 * pending, with the not-yet-tokenized tail appended uncolored so text never
 * lags behind the stream either.
 *
 * Reuse is guarded on this block's own state (the hook is per-block), on the
 * language being unchanged, and on the old code still being a prefix of the
 * new one — a fence that switches language or is rewritten falls back to raw
 * rather than painting one language's colors onto another's source.
 */
function useHighlightedCode(
  code: string,
  language: string,
  highlighter: CodeHighlighterPlugin,
): HighlightResult {
  const cacheKey = `${language}\u0000${code}`;
  const [highlighted, setHighlighted] = useState<HighlightedEntry | null>(null);

  useEffect(() => {
    let active = true;
    const accept = (result: HighlightResult) => {
      if (active) setHighlighted({ cacheKey, code, language, result });
    };
    const result = highlighter.highlight(
      {
        code,
        language: language as BundledLanguage,
        themes: highlighter.getThemes(),
      },
      accept,
    );
    if (result) accept(result);
    return () => {
      active = false;
    };
  }, [cacheKey, code, highlighter, language]);

  return useMemo(() => {
    if (highlighted?.cacheKey === cacheKey) return highlighted.result;
    if (highlighted?.language === language && code.startsWith(highlighted.code)) {
      return withPlainTail(
        highlighted.result,
        code.slice(highlighted.code.length),
      );
    }
    return rawHighlightResult(code);
  }, [cacheKey, code, highlighted, language]);
}

type TokenStyle = CSSProperties & Record<`--${string}`, string | number>;

function tokenStyle(token: HighlightResult["tokens"][number][number]): TokenStyle {
  const style: TokenStyle = { ...token.htmlStyle };
  if (token.color) style["--chat-code-token"] = token.color;
  if (token.bgColor) style["--chat-code-token-bg"] = token.bgColor;
  return style;
}

function HighlightedCode({
  code,
  language,
  highlighter,
}: {
  code: string;
  language: string;
  highlighter: CodeHighlighterPlugin;
}) {
  const result = useHighlightedCode(code, language, highlighter);
  const lines = useMemo(
    () =>
      result.tokens.map((tokens, lineIndex) => ({
        key: `line-${lineIndex}-${tokens[0]?.offset ?? 0}`,
        tokens: tokens.map((token, tokenIndex) => ({
          key: `token-${tokenIndex}-${token.offset ?? 0}`,
          token,
        })),
      })),
    [result.tokens],
  );

  return (
    <pre
      style={{
        backgroundColor: result.bg ?? "transparent",
        color: result.fg ?? "inherit",
      }}
    >
      <code>
        {lines.map((line) => (
          <span key={line.key} data-chat-code-line="">
            {line.tokens.length === 0
              ? "\n"
              : line.tokens.map(({ key, token }) => (
                  <span
                    key={key}
                    className="text-[var(--chat-code-token,inherit)] dark:text-[var(--shiki-dark,var(--chat-code-token,inherit))] bg-[var(--chat-code-token-bg,transparent)] dark:bg-[var(--shiki-dark-bg,var(--chat-code-token-bg,transparent))]"
                    style={tokenStyle(token)}
                    {...token.htmlAttrs}
                  >
                    {token.content}
                  </span>
                ))}
          </span>
        ))}
      </code>
    </pre>
  );
}

function CodeBlockTitle({
  fenceTitle,
  language,
}: {
  fenceTitle: string | null;
  language: string;
}) {
  const { cwd, workspaceId } = useChatFileLinkContext();
  const source = fenceTitle ? resolveChatFileLink(fenceTitle, cwd) : null;
  if (fenceTitle) {
    return (
      <span data-chat-code-title="" title={fenceTitle}>
        {source && workspaceId ? (
          <MarkdownFileLink meta={source} variant="plain">
            {fenceTitle}
          </MarkdownFileLink>
        ) : (
          <>
            <FileTypeIcon filename={fenceTitle} className="size-3.5" />
            <span className="truncate">{fenceTitle}</span>
          </>
        )}
      </span>
    );
  }

  const syntheticFilename = syntheticFilenameForLanguage(language);
  if (!hasSpecificFileTypeIcon(syntheticFilename)) {
    return <span data-chat-code-title="">{language}</span>;
  }

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          data-chat-code-title=""
          aria-label={`Language: ${language}`}
          tabIndex={0}
        >
          <FileTypeIcon filename={syntheticFilename} className="size-3.5" />
        </span>
      </TooltipTrigger>
      <TooltipContent side="top">{language}</TooltipContent>
    </Tooltip>
  );
}

function ChatFencedCode({
  children,
  className,
  node,
}: ComponentProps<"code"> & ExtraProps) {
  const { defaultWrap, highlighter } = useChatCodeRenderer();
  const isIncomplete = useIsCodeFenceIncomplete();
  const language = extractFenceLanguage(className);
  const meta = node?.properties?.metastring;
  const fenceTitle = extractFenceTitle(
    typeof meta === "string" ? meta : undefined,
  );
  // mdast terminates a fence's value with exactly one newline; strip that one
  // and no more, so a snippet that genuinely ends in blank lines keeps them in
  // both the render and the clipboard.
  const code = nodeToPlainText(children).replace(/\n$/, "");
  const [wrapOverride, setWrapOverride] = useState<boolean | null>(null);
  const wrapped = wrapOverride ?? defaultWrap;
  const [copied, setCopied] = useState(false);
  const copiedTimer = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (copiedTimer.current !== null) window.clearTimeout(copiedTimer.current);
    },
    [],
  );

  async function copyCode() {
    if (!navigator.clipboard?.writeText || copied) return;
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      if (copiedTimer.current !== null) window.clearTimeout(copiedTimer.current);
      copiedTimer.current = window.setTimeout(() => {
        setCopied(false);
        copiedTimer.current = null;
      }, 1200);
    } catch (error) {
      console.error("Failed to copy chat code block", error);
    }
  }

  const wrapLabel = wrapped ? "Disable line wrap" : "Wrap lines";
  const copyLabel = copied ? "Copied" : "Copy code";

  return (
    <div
      data-chat-code-block=""
      data-incomplete={isIncomplete || undefined}
      data-language={language}
      data-wrap={wrapped ? "true" : "false"}
    >
      <div data-chat-code-header="">
        <CodeBlockTitle fenceTitle={fenceTitle} language={language} />
        <span data-chat-code-actions="">
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                aria-label={wrapLabel}
                aria-pressed={wrapped}
                onClick={() => setWrapOverride(!wrapped)}
              >
                <WrapTextIcon />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="top">{wrapLabel}</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                aria-label={copyLabel}
                onClick={() => void copyCode()}
              >
                {copied ? <CheckIcon /> : <CopyIcon />}
              </Button>
            </TooltipTrigger>
            <TooltipContent side="top">{copyLabel}</TooltipContent>
          </Tooltip>
        </span>
      </div>
      <div data-chat-code-body="">
        <HighlightedCode
          code={code}
          language={language}
          highlighter={highlighter}
        />
      </div>
    </div>
  );
}

function ChatInlineCode({
  children,
  className,
  node: _node,
  ...props
}: ComponentProps<"code"> & ExtraProps) {
  return (
    <code
      {...props}
      className={className}
      data-streamdown="inline-code"
    >
      {children}
    </code>
  );
}

export const CHAT_MARKDOWN_COMPONENTS: Components = {
  code: ChatFencedCode,
  inlineCode: ChatInlineCode,
};
