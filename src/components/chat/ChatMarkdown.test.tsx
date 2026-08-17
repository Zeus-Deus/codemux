import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import { useSettingsStore } from "@/stores/settings-store";
import {
  extractFenceTitle,
  syntheticFilenameForLanguage,
} from "./ChatCodeBlock";
import { ChatMarkdown } from "./ChatMarkdown";

const codePluginMocks = vi.hoisted(() => {
  // Mirrors `@streamdown/code`: answer synchronously only for code already
  // seen, otherwise return null and resolve through the callback. Tests drive
  // `deferred` to hold a highlight open the way a cold Shiki load would.
  let deferred = false;
  const pending: Array<() => void> = [];
  const seen = new Set<string>();

  const tokenize = ({ code }: { code: string }) => ({
    bg: "transparent",
    fg: "inherit",
    tokens: code.split("\n").map((line) => [
      {
        content: line,
        color: "rgb(1, 2, 3)",
        bgColor: "transparent",
        htmlStyle: {},
        offset: 0,
      },
    ]),
  });

  const highlight = vi.fn(
    (
      options: { code: string; language: string },
      callback?: (result: ReturnType<typeof tokenize>) => void,
    ) => {
      const key = `${options.language} ${options.code}`;
      if (!deferred || seen.has(key)) {
        seen.add(key);
        return tokenize(options);
      }
      pending.push(() => {
        seen.add(key);
        callback?.(tokenize(options));
      });
      return null;
    },
  );

  return {
    setDeferred: (value: boolean) => {
      deferred = value;
    },
    flush: () => {
      const queued = pending.splice(0, pending.length);
      for (const resolve of queued) resolve();
    },
    reset: () => {
      deferred = false;
      pending.length = 0;
      seen.clear();
    },
    highlighter: {
      name: "shiki",
      type: "code-highlighter",
      getSupportedLanguages: () => ["text", "tsx"],
      getThemes: () => ["github-light", "github-dark"],
      supportsLanguage: () => true,
      highlight,
    },
    highlight,
  };
});

vi.mock("@/hooks/use-chat-code-plugin", () => ({
  useChatCodePlugin: () => codePluginMocks.highlighter,
}));

function fenced(language: string, code: string, meta = ""): string {
  return [`\`\`\`${language}${meta ? ` ${meta}` : ""}`, code, "```"].join(
    "\n",
  );
}

function markdownTree(markdown: string) {
  return (
    <TooltipProvider>
      <ChatMarkdown>{markdown}</ChatMarkdown>
    </TooltipProvider>
  );
}

function renderMarkdown(markdown: string) {
  return render(markdownTree(markdown));
}

/**
 * Put a working async clipboard in place. jsdom leaves `isSecureContext`
 * undefined, which the shared copy helper reads as "insecure origin" and
 * routes past `navigator.clipboard` into its execCommand fallback.
 */
function stubClipboard() {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(window, "isSecureContext", {
    configurable: true,
    value: true,
  });
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText },
  });
  return writeText;
}

/** Per-token highlight color, in source order — `inherit` means un-highlighted. */
function tokenColors(container: HTMLElement): string[] {
  return Array.from(
    container.querySelectorAll<HTMLElement>("[data-chat-code-line] span"),
  ).map((span) => span.style.getPropertyValue("--chat-code-token"));
}

beforeEach(() => {
  useSettingsStore.setState({ loaded: true, settings: {} });
  codePluginMocks.highlight.mockClear();
  codePluginMocks.reset();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("ChatMarkdown code blocks", () => {
  it("renders a language-specific icon with an accessible language label", () => {
    const { container } = renderMarkdown(
      fenced("tsx", "const App = () => <main />;"),
    );

    expect(screen.getByLabelText("Language: tsx")).toBeInTheDocument();
    const icon = container.querySelector("[data-chat-code-title] [data-file-icon]");
    expect(icon).not.toHaveAttribute("data-file-icon", "file");
  });

  it("renders a filename and its icon from supported fence metadata", () => {
    const { container } = renderMarkdown(
      fenced("tsx", "export function Composer() {}", 'title="src/chat/Composer.tsx"'),
    );

    expect(screen.getByText("src/chat/Composer.tsx")).toBeInTheDocument();
    expect(
      container.querySelector('[data-chat-code-title] [data-file-icon]'),
    ).not.toHaveAttribute("data-file-icon", "file");
  });

  it("falls back to a textual language label when no specific icon exists", () => {
    renderMarkdown(fenced("madeuplang", "beep boop"));
    expect(screen.getByText("madeuplang")).toBeInTheDocument();
  });

  it("uses the global wrap preference until that block is overridden", () => {
    useSettingsStore.setState({
      loaded: true,
      settings: { "chat.code_wrap": "true" },
    });
    const { container } = renderMarkdown(fenced("tsx", "const longLine = true;"));
    const block = container.querySelector("[data-chat-code-block]");

    expect(block).toHaveAttribute("data-wrap", "true");
    fireEvent.click(screen.getByRole("button", { name: "Disable line wrap" }));
    expect(block).toHaveAttribute("data-wrap", "false");
  });

  it("copies code without the Markdown fence", async () => {
    const writeText = stubClipboard();
    renderMarkdown(fenced("tsx", "const answer = 42;"));

    fireEvent.click(screen.getByRole("button", { name: "Copy code" }));
    await vi.waitFor(() => {
      expect(writeText).toHaveBeenCalledWith("const answer = 42;");
      expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
    });
  });

  it("keeps blank lines a snippet genuinely ends with", async () => {
    const writeText = stubClipboard();
    // mdast terminates the fence value with one newline; the two blank lines
    // before it are the author's and must survive render and copy.
    const { container } = renderMarkdown(fenced("txt", "one\n\n"));

    expect(container.querySelectorAll("[data-chat-code-line]")).toHaveLength(3);
    fireEvent.click(screen.getByRole("button", { name: "Copy code" }));
    await vi.waitFor(() => {
      expect(writeText).toHaveBeenCalledWith("one\n\n");
    });
  });

  it("still copies on an origin without the async clipboard", async () => {
    // The remote web client can be plain HTTP, where `navigator.clipboard` is
    // undefined; the code block goes through the same fallback as the
    // message-level copy rather than silently doing nothing.
    Object.defineProperty(window, "isSecureContext", {
      configurable: true,
      value: false,
    });
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: undefined,
    });
    const execCommand = vi.fn().mockReturnValue(true);
    Object.defineProperty(document, "execCommand", {
      configurable: true,
      value: execCommand,
    });
    renderMarkdown(fenced("tsx", "const answer = 42;"));

    fireEvent.click(screen.getByRole("button", { name: "Copy code" }));
    await vi.waitFor(() => {
      expect(execCommand).toHaveBeenCalledWith("copy");
      expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
    });
  });

  it("keeps plain fences identifiable and actionable", () => {
    renderMarkdown(fenced("", "plain output"));
    expect(screen.getByText("text")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Wrap lines" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy code" })).toBeInTheDocument();
  });
});

describe("streaming code blocks", () => {
  it("keeps the previous highlight on screen while an append re-highlights", () => {
    codePluginMocks.setDeferred(true);
    const { container, rerender } = renderMarkdown(
      fenced("tsx", "const a = 1;"),
    );
    act(() => codePluginMocks.flush());
    expect(tokenColors(container)).toEqual(["rgb(1, 2, 3)"]);

    rerender(markdownTree(fenced("tsx", "const a = 1;\nconst b = 2;")));

    // No raw fallback: the already-highlighted line keeps its color and the
    // streamed tail rides along un-highlighted until Shiki catches up.
    expect(tokenColors(container)).toEqual(["rgb(1, 2, 3)", "inherit"]);
    expect(container.textContent).toContain("const b = 2;");

    act(() => codePluginMocks.flush());
    expect(tokenColors(container)).toEqual(["rgb(1, 2, 3)", "rgb(1, 2, 3)"]);
  });

  it("drops to raw rather than reusing another language's highlight", () => {
    codePluginMocks.setDeferred(true);
    const { container, rerender } = renderMarkdown(fenced("tsx", "let x = 1;"));
    act(() => codePluginMocks.flush());
    expect(tokenColors(container)).toEqual(["rgb(1, 2, 3)"]);

    rerender(markdownTree(fenced("python", "let x = 1;")));
    expect(tokenColors(container)).toEqual(["inherit"]);
  });
});

describe("chat code fence metadata", () => {
  it("accepts title, file, filename, and bare filename forms", () => {
    expect(extractFenceTitle('title="src/App.tsx"')).toBe("src/App.tsx");
    expect(extractFenceTitle("file='src/main.rs'")).toBe("src/main.rs");
    expect(extractFenceTitle("filename=worker.py")).toBe("worker.py");
    expect(extractFenceTitle("noLineNumbers src/theme.css")).toBe("src/theme.css");
  });

  it("treats real paths and dotfiles as filenames", () => {
    expect(extractFenceTitle("main.rs")).toBe("main.rs");
    expect(extractFenceTitle(".env.local")).toBe(".env.local");
    expect(extractFenceTitle("src/lib/foo.ts")).toBe("src/lib/foo.ts");
    expect(extractFenceTitle("@scope/pkg/file.tsx")).toBe("@scope/pkg/file.tsx");
    expect(extractFenceTitle("Dockerfile.prod")).toBe("Dockerfile.prod");
    expect(extractFenceTitle("v2.config.js")).toBe("v2.config.js");
  });

  it("ignores version numbers and line ranges that merely contain a dot", () => {
    expect(extractFenceTitle("1.5")).toBeNull();
    expect(extractFenceTitle("v2.0")).toBeNull();
    expect(extractFenceTitle("2.0.x")).toBeNull();
    expect(extractFenceTitle("{1-3}")).toBeNull();
    expect(extractFenceTitle("showLineNumbers {1-3}")).toBeNull();
  });

  it("maps common fence aliases to filenames understood by the icon set", () => {
    expect(syntheticFilenameForLanguage("typescript")).toBe("file.ts");
    expect(syntheticFilenameForLanguage("tsx")).toBe("file.tsx");
    expect(syntheticFilenameForLanguage("bash")).toBe("file.sh");
    expect(syntheticFilenameForLanguage("dockerfile")).toBe("Dockerfile");
  });
});
