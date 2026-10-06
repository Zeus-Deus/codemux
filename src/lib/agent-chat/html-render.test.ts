import { describe, expect, it, vi } from "vitest";

import { BUILT_IN_THEMES } from "@/lib/themes";

import {
  buildHtmlRenderDocument,
  htmlRenderTheme,
  isInlineHtmlRender,
  readHtmlRender,
  readHtmlRenderContentHeight,
  readHtmlRenderLinkRequest,
} from "./html-render";
import type { ToolCallItem } from "./types";

function call(overrides: Partial<ToolCallItem>): ToolCallItem {
  return {
    kind: "tool_call",
    id: "tc-1",
    seq: 1,
    tool_use_id: "tu-1",
    tool_name: "mcp__codemux__mcp__codemux__html_render",
    input: { title: "Chart", html: "<p>hi</p>" },
    status: "done",
    result_content: null,
    approval_request_id: null,
    ...overrides,
  };
}

const FONTS = { sans: "Sans", mono: "Mono" };

describe("readHtmlRender", () => {
  it("reads a Claude-style call", () => {
    expect(readHtmlRender(call({ input: { title: " Chart ", html: "<p>x</p>", height: 5000 } }))).toEqual({
      title: "Chart",
      html: "<p>x</p>",
      maxHeight: 2000,
    });
  });

  it("unwraps a Codex dynamic tool call, including string arguments", () => {
    const item = call({
      tool_name: "dynamicToolCall",
      input: {
        type: "dynamicToolCall",
        tool: "codemux_mcp__codemux__html_render",
        arguments: JSON.stringify({ title: "Chart", html: "<p>x</p>" }),
      },
    });
    expect(readHtmlRender(item)?.html).toBe("<p>x</p>");
  });

  it("ignores other tools, including a same-named tool on another server", () => {
    expect(readHtmlRender(call({ tool_name: "Read" }))).toBeNull();
    expect(readHtmlRender(call({ tool_name: "mcp__other__html_render" }))).toBeNull();
    expect(readHtmlRender(call({ tool_name: "mcp__my-codemux__html_render" }))).toBeNull();
    expect(readHtmlRender(call({ tool_name: "mcp__my_codemux__html_render" }))).toBeNull();
  });

  it("ignores a call without a page", () => {
    expect(readHtmlRender(call({ input: { title: "Chart" } }))).toBeNull();
  });
});

describe("isInlineHtmlRender", () => {
  it("shows running and successful calls inline, failed ones as tool steps", () => {
    expect(isInlineHtmlRender(call({ status: "running", input: {} }))).toBe(true);
    expect(isInlineHtmlRender(call({}))).toBe(true);
    expect(isInlineHtmlRender(call({ status: "error" }))).toBe(false);
    expect(isInlineHtmlRender(call({ tool_name: "Bash", status: "running" }))).toBe(false);
  });
});

describe("htmlRenderTheme", () => {
  it("maps the palette to the variables agents are told about", () => {
    const theme = BUILT_IN_THEMES[0]!;
    const { scheme, variables } = htmlRenderTheme(theme, FONTS);
    expect(scheme).toBe(theme.scheme);
    expect(variables["--background"]).toBe(theme.roles.background);
    expect(variables["--accent"]).toBe(theme.roles.brandAccent);
    expect(variables["--chart-1"]).toBe(theme.roles.brandAccent);
    expect(variables["--destructive"]).toBe(theme.ansi.red);
    expect(variables["--font-sans"]).toBe("Sans");
    for (let index = 1; index <= 6; index++) {
      expect(variables[`--chart-${index}`]).toBeTruthy();
    }
  });
});

describe("buildHtmlRenderDocument", () => {
  const theme = htmlRenderTheme(BUILT_IN_THEMES[0]!, FONTS);

  it("inserts the bootstrap at the start of the head", () => {
    const html = buildHtmlRenderDocument("<!doctype html><html><head><title>x</title></head><body></body></html>", theme);
    expect(html.indexOf('<style id="codemux-theme">')).toBeGreaterThan(html.indexOf("<head>"));
    expect(html.indexOf('<style id="codemux-theme">')).toBeLessThan(html.indexOf("<title>"));
  });

  it("does not inject into a <head> inside a script or comment", () => {
    const page = '<!-- <head> --><script>const s = "<head>";</script><p>x</p>';
    const html = buildHtmlRenderDocument(page, theme);
    expect(html.startsWith("<!doctype html><head>")).toBe(true);
    expect(html.endsWith(page)).toBe(true);
  });

  it("strips characters that could break out of the theme rule", () => {
    const hostile = { ...theme, variables: { "--background": "red;}</style><script>alert(1)</script>" } };
    expect(buildHtmlRenderDocument("<p>x</p>", hostile)).not.toContain("</style><script>alert");
  });
});

describe("bridge messages", () => {
  it("reads a page's reported height", () => {
    const message = { jsonrpc: "2.0", method: "ui/notifications/size-changed", params: { height: 420 } };
    expect(readHtmlRenderContentHeight(message)).toBe(420);
    expect(readHtmlRenderContentHeight({ ...message, params: { height: -1 } })).toBeUndefined();
  });

  it("only accepts http(s) link requests", () => {
    const message = (url: string) => ({ jsonrpc: "2.0", method: "ui/open-link", params: { url } });
    expect(readHtmlRenderLinkRequest(message("https://example.com"))).toBe("https://example.com");
    expect(readHtmlRenderLinkRequest(message("file:///etc/passwd"))).toBeUndefined();
  });
});

describe("bootstrap link handling", () => {
  it("scrolls #fragment links in place and forwards only http(s) links", async () => {
    const page = buildHtmlRenderDocument("<p>x</p>", htmlRenderTheme(BUILT_IN_THEMES[0]!, FONTS));
    const script = /<script>([\s\S]*?)<\/script>/.exec(page)![1]!;
    // jsdom is the top window here, so the page's posts land on this window.
    new Function(script)();
    const posted: unknown[] = [];
    const onMessage = (event: MessageEvent) => {
      if (readHtmlRenderLinkRequest(event.data)) posted.push(event.data);
    };
    window.addEventListener("message", onMessage);
    document.body.innerHTML =
      '<a id="frag" href="#target">jump</a><a id="mail" href="mailto:a@example.com">mail</a>' +
      '<a id="ext" href="https://example.com/docs">ext</a><h2 id="target">T</h2>';
    const target = document.getElementById("target")!;
    target.scrollIntoView = vi.fn();
    const click = (id: string) => {
      const event = new MouseEvent("click", { bubbles: true, cancelable: true });
      document.getElementById(id)!.dispatchEvent(event);
      return event.defaultPrevented;
    };

    expect(click("frag")).toBe(true);
    expect(target.scrollIntoView).toHaveBeenCalledOnce();
    expect(click("mail")).toBe(true);
    expect(click("ext")).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    window.removeEventListener("message", onMessage);
    document.body.innerHTML = "";

    expect(posted).toEqual([
      { jsonrpc: "2.0", method: "ui/open-link", params: { url: "https://example.com/docs" } },
    ]);
  });
});
