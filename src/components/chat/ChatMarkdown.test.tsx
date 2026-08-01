import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import { useSettingsStore } from "@/stores/settings-store";
import {
  extractFenceTitle,
  syntheticFilenameForLanguage,
} from "./ChatCodeBlock";
import { ChatMarkdown } from "./ChatMarkdown";

const codePluginMocks = vi.hoisted(() => {
  const highlight = vi.fn(
    ({ code }: { code: string }) => ({
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
    }),
  );
  return {
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

function renderMarkdown(markdown: string) {
  return render(
    <TooltipProvider>
      <ChatMarkdown>{markdown}</ChatMarkdown>
    </TooltipProvider>,
  );
}

beforeEach(() => {
  useSettingsStore.setState({ loaded: true, settings: {} });
  codePluginMocks.highlight.mockClear();
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
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    renderMarkdown(fenced("tsx", "const answer = 42;"));

    fireEvent.click(screen.getByRole("button", { name: "Copy code" }));
    await vi.waitFor(() => {
      expect(writeText).toHaveBeenCalledWith("const answer = 42;");
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

describe("chat code fence metadata", () => {
  it("accepts title, file, filename, and bare filename forms", () => {
    expect(extractFenceTitle('title="src/App.tsx"')).toBe("src/App.tsx");
    expect(extractFenceTitle("file='src/main.rs'")).toBe("src/main.rs");
    expect(extractFenceTitle("filename=worker.py")).toBe("worker.py");
    expect(extractFenceTitle("noLineNumbers src/theme.css")).toBe("src/theme.css");
  });

  it("maps common fence aliases to filenames understood by the icon set", () => {
    expect(syntheticFilenameForLanguage("typescript")).toBe("file.ts");
    expect(syntheticFilenameForLanguage("tsx")).toBe("file.tsx");
    expect(syntheticFilenameForLanguage("bash")).toBe("file.sh");
    expect(syntheticFilenameForLanguage("dockerfile")).toBe("Dockerfile");
  });
});
