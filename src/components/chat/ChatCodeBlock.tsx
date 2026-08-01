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
const FENCE_FILENAME_TOKEN_REGEX = /^[\w@][\w@./-]*\.[A-Za-z0-9]+$/;

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
  return (
    meta
      .split(/\s+/)
      .find((candidate) => FENCE_FILENAME_TOKEN_REGEX.test(candidate)) ?? null
  );
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

function rawHighlightResult(code: string): HighlightResult {
  return {
    bg: "transparent",
    fg: "inherit",
    tokens: code.split("\n").map((line) => [
      {
        content: line,
        color: "inherit",
        bgColor: "transparent",
        htmlStyle: {},
        offset: 0,
      },
    ]),
  };
}

function useHighlightedCode(
  code: string,
  language: string,
  highlighter: CodeHighlighterPlugin,
): HighlightResult {
  const cacheKey = `${language}\u0000${code}`;
  const raw = useMemo(() => rawHighlightResult(code), [code]);
  const [highlighted, setHighlighted] = useState<{
    cacheKey: string;
    result: HighlightResult;
  } | null>(null);

  useEffect(() => {
    let active = true;
    const accept = (result: HighlightResult) => {
      if (active) setHighlighted({ cacheKey, result });
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

  return highlighted?.cacheKey === cacheKey ? highlighted.result : raw;
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
  if (fenceTitle) {
    return (
      <span data-chat-code-title="" title={fenceTitle}>
        <FileTypeIcon filename={fenceTitle} className="size-3.5" />
        <span className="truncate">{fenceTitle}</span>
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

type MarkdownCodeProps = ComponentProps<"code"> &
  ExtraProps & {
    metastring?: string;
    "data-block"?: string;
  };

function ChatFencedCode({
  children,
  className,
  node,
}: MarkdownCodeProps) {
  const { defaultWrap, highlighter } = useChatCodeRenderer();
  const isIncomplete = useIsCodeFenceIncomplete();
  const language = extractFenceLanguage(className);
  const meta = node?.properties?.metastring;
  const fenceTitle = extractFenceTitle(
    typeof meta === "string" ? meta : undefined,
  );
  const code = nodeToPlainText(children).replace(/\n+$/, "");
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
