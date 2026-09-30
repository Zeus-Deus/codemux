import type { CodeHighlighterPlugin, HighlightResult, ThemeInput } from "@streamdown/code";
import { act, cleanup, render } from "@testing-library/react";
import { Profiler, type ComponentType } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import { CHAT_MARKDOWN_COMPONENTS, ChatCodeRendererProvider } from "./ChatCodeBlock";

vi.mock("streamdown", () => ({ useIsCodeFenceIncomplete: () => false }));
afterEach(cleanup);

const Fence = CHAT_MARKDOWN_COMPONENTS.code as ComponentType<{
  className: string;
  children: string;
}>;

function tokens(code: string, color = "#ff0000"): HighlightResult {
  return {
    fg: color,
    bg: "transparent",
    tokens: code.split("\n").map(content => [{ content, color, offset: 0, htmlStyle: {} }]),
  };
}

function plugin(): CodeHighlighterPlugin {
  return {
    name: "shiki",
    type: "code-highlighter",
    getThemes: () => ["github-light", "github-dark"],
    getSupportedLanguages: () => ["typescript"],
    supportsLanguage: () => true,
    highlight: vi.fn(({ code }) => tokens(code)),
  };
}

function tree(code: string, highlighter: CodeHighlighterPlugin, commits: string[] = []) {
  return (
    <TooltipProvider>
      <ChatCodeRendererProvider defaultWrap={false} highlighter={highlighter}>
        <Profiler id="fence" onRender={() => {
          commits.push(document.querySelector("[data-chat-code-body] span span")?.getAttribute("style") ?? "");
        }}>
          <Fence className="language-typescript">{code}</Fence>
        </Profiler>
      </ChatCodeRendererProvider>
    </TooltipProvider>
  );
}

function mount(code: string, highlighter: CodeHighlighterPlugin, commits: string[] = []) {
  return render(tree(code, highlighter, commits));
}

function lastColor(container: HTMLElement) {
  return container.querySelector("[data-chat-code-body] span span")?.getAttribute("style");
}

describe("code fence remounts", () => {
  it("paints cached highlight tokens in the first commit on remount", () => {
    const highlighter = plugin();
    const first: string[] = [];
    const a = mount("const value = 1;", highlighter, first);
    expect(first[first.length - 1]).toContain("#ff0000");
    a.unmount();
    vi.mocked(highlighter.highlight).mockClear();
    const second: string[] = [];
    mount("const value = 1;", highlighter, second);
    expect(second[0]).toContain("#ff0000");
    // Tooltip registration can commit independently; none of those commits
    // should substitute raw tokens for the already completed highlight.
    expect(second.every(style => style.includes("#ff0000"))).toBe(true);
    expect(highlighter.highlight).not.toHaveBeenCalled();
  });

  it("does not reuse another highlighter's colors", () => {
    mount("const value = 1;", plugin()).unmount();
    const other = plugin();
    vi.mocked(other.highlight).mockImplementation(({ code }) => tokens(code, "#0000ff"));
    const b = mount("const value = 1;", other);
    expect(lastColor(b.container)).toContain("#0000ff");
    expect(other.highlight).toHaveBeenCalledOnce();
  });

  it("invalidates cached colors when a plugin's themes change", () => {
    let themes: [ThemeInput, ThemeInput] = ["github-light", "github-dark"];
    const highlighter = plugin();
    highlighter.getThemes = () => themes;
    mount("const value = 1;", highlighter).unmount();
    themes = ["light-plus", "dark-plus"];
    vi.mocked(highlighter.highlight).mockImplementation(({ code }) => tokens(code, "#0000ff"));
    const commits: string[] = [];
    const b = mount("const value = 1;", highlighter, commits);
    expect(commits[0]).not.toContain("#ff0000");
    expect(lastColor(b.container)).toContain("#0000ff");
    expect(highlighter.highlight).toHaveBeenCalledTimes(2);
  });

  it("keeps arriving text visible and ignores late results after unmount", () => {
    const pending: Array<(result: HighlightResult) => void> = [];
    const highlighter = plugin();
    vi.mocked(highlighter.highlight).mockImplementation((_options, callback) => {
      if (callback) pending.push(callback);
      return null;
    });
    const a = mount("const value = 1;", highlighter);
    expect(a.container.querySelector("code")?.textContent).toContain("const value = 1;");
    a.unmount();
    const b = mount("const value = 2;", highlighter);
    act(() => pending[0](tokens("const value = 1;")));
    expect(b.container.querySelector("code")?.textContent).toContain("const value = 2;");
    act(() => pending[1](tokens("const value = 2;")));
    expect(lastColor(b.container)).toContain("#ff0000");
  });

  it("preserves highlighted prefixes and immediate plain tails during streaming", () => {
    const highlighter = plugin();
    const original = "const value = 1;";
    mount(original, highlighter).unmount();
    const b = mount(original, highlighter);
    let complete: ((result: HighlightResult) => void) | undefined;
    vi.mocked(highlighter.highlight).mockImplementation((_options, callback) => {
      complete = callback;
      return null;
    });
    const appended = `${original}\nconst next = 2;`;
    b.rerender(tree(appended, highlighter));
    expect(b.container.querySelector("code")?.textContent).toContain("const next = 2;");
    expect(lastColor(b.container)).toContain("#ff0000");
    act(() => complete?.(tokens(appended, "#0000ff")));
    expect(lastColor(b.container)).toContain("#0000ff");
    b.rerender(tree("rewritten", highlighter));
    expect(lastColor(b.container)).not.toContain("#0000ff");
    expect(b.container.querySelector("code")?.textContent).toBe("rewritten");
  });

  it("reads a cached prop update without another highlight request", () => {
    const highlighter = plugin();
    mount("first", highlighter).unmount();
    mount("second", highlighter).unmount();
    const a = mount("first", highlighter);
    vi.mocked(highlighter.highlight).mockClear();
    a.rerender(tree("second", highlighter));
    expect(a.container.querySelector("code")?.textContent).toBe("second");
    expect(lastColor(a.container)).toContain("#ff0000");
    expect(highlighter.highlight).not.toHaveBeenCalled();
  });

  it("ignores obsolete mounted-code callbacks without caching their result", () => {
    const pending: Array<(result: HighlightResult) => void> = [];
    const highlighter = plugin();
    vi.mocked(highlighter.highlight).mockImplementation((_options, callback) => {
      if (callback) pending.push(callback);
      return null;
    });
    const a = mount("first", highlighter);
    a.rerender(tree("second", highlighter));
    act(() => pending[0](tokens("first")));
    expect(a.container.querySelector("code")?.textContent).toBe("second");
    expect(lastColor(a.container)).not.toContain("#ff0000");
    act(() => pending[1](tokens("second", "#0000ff")));
    expect(lastColor(a.container)).toContain("#0000ff");
    a.unmount();
    const b = mount("first", highlighter);
    expect(lastColor(b.container)).not.toContain("#ff0000");
    expect(highlighter.highlight).toHaveBeenCalledTimes(3);
  });

  it("does not cache a late callback from a replaced plugin", () => {
    let obsolete: ((result: HighlightResult) => void) | undefined;
    const oldPlugin = plugin();
    vi.mocked(oldPlugin.highlight).mockImplementation((_options, callback) => {
      obsolete = callback;
      return null;
    });
    const nextPlugin = plugin();
    vi.mocked(nextPlugin.highlight).mockImplementation(({ code }) => tokens(code, "#0000ff"));
    const a = mount("same source", oldPlugin);
    a.rerender(tree("same source", nextPlugin));
    act(() => obsolete?.(tokens("same source")));
    expect(lastColor(a.container)).toContain("#0000ff");
    a.unmount();
    const b = mount("same source", oldPlugin);
    expect(lastColor(b.container)).not.toContain("#ff0000");
    expect(oldPlugin.highlight).toHaveBeenCalledTimes(2);
  });

  it("does not cache a late callback for obsolete mounted themes", () => {
    const pending: Array<(result: HighlightResult) => void> = [];
    let themes: [ThemeInput, ThemeInput] = ["github-light", "github-dark"];
    const highlighter = plugin();
    highlighter.getThemes = () => themes;
    vi.mocked(highlighter.highlight).mockImplementation((_options, callback) => {
      if (callback) pending.push(callback);
      return null;
    });
    const a = mount("same source", highlighter);
    themes = ["light-plus", "dark-plus"];
    a.rerender(tree("same source", highlighter));
    act(() => pending[0](tokens("same source")));
    expect(lastColor(a.container)).not.toContain("#ff0000");
    act(() => pending[1](tokens("same source", "#0000ff")));
    expect(lastColor(a.container)).toContain("#0000ff");
    a.unmount();
    themes = ["github-light", "github-dark"];
    const b = mount("same source", highlighter);
    expect(lastColor(b.container)).not.toContain("#ff0000");
    expect(highlighter.highlight).toHaveBeenCalledTimes(3);
  });

  it("bounds retained entries instead of keeping every visited fence", () => {
    const highlighter = plugin();
    for (let i = 0; i < 65; i++) mount(`value ${i}`, highlighter).unmount();
    vi.mocked(highlighter.highlight).mockClear();
    mount("value 0", highlighter);
    expect(highlighter.highlight).toHaveBeenCalledOnce();
  });

  it("bounds retained source characters even when the entry count is small", () => {
    const highlighter = plugin();
    const code = "x".repeat(40_000);
    for (let i = 0; i < 7; i++) mount(`${i}${code}`, highlighter).unmount();
    vi.mocked(highlighter.highlight).mockClear();
    mount(`0${code}`, highlighter);
    expect(highlighter.highlight).toHaveBeenCalledOnce();
  });
});
