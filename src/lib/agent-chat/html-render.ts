import { contrastRatio, normalizeColor, type ThemeDefinition } from "@/lib/themes";

import type { ToolCallItem } from "./types";

/**
 * Agent HTML renders ("visual replies"): self-contained pages an agent shows
 * inline in a chat thread with the `html_render` tool on Codemux's own MCP
 * server (`src-tauri/src/mcp_server.rs`). The page travels in the tool call's
 * input, so the transcript already persists it. The client shows it in a
 * sandboxed frame and hands it the active theme as CSS custom properties.
 */

export const HTML_RENDER_MIN_HEIGHT = 80;
export const HTML_RENDER_MAX_HEIGHT = 2000;
/** The frame's height before the page reports its own. */
export const HTML_RENDER_INITIAL_HEIGHT = 320;

export interface HtmlRender {
  title: string;
  html: string;
  /** The agent's cap on the frame height, when it asked for one. */
  maxHeight: number | null;
}

// Claude sees Codemux tools as `mcp__codemux__mcp__codemux__html_render`,
// Codex as `codemux_mcp__codemux__html_render`; both end in the server-scoped
// name after a `__` separator. A same-named tool on another MCP server
// (`mcp__my-codemux__html_render`, `mcp__other__html_render`) does not match.
const TOOL_NAME = /(?:^|__)codemux__html_render$/;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The tool's own name and arguments. Codex wraps dynamic tool calls in an
 *  item envelope (`{ tool, arguments }`); other providers send them as is. */
function toolCall(item: ToolCallItem): { name: string; args: unknown } {
  const input = item.input;
  if (
    (item.tool_name === "dynamicToolCall" || item.tool_name === "mcpToolCall") &&
    isRecord(input) &&
    typeof input.tool === "string"
  ) {
    let args = input.arguments;
    if (typeof args === "string") {
      try {
        args = JSON.parse(args);
      } catch {
        args = null;
      }
    }
    return { name: input.tool, args };
  }
  return { name: item.tool_name, args: input };
}

/** Whether a tool call is an `html_render` call, whatever its state. */
export function isHtmlRenderTool(item: ToolCallItem): boolean {
  return TOOL_NAME.test(toolCall(item).name);
}

/** The page an `html_render` call shows, or `null` while its input is
 *  incomplete or for any other tool. */
export function readHtmlRender(item: ToolCallItem): HtmlRender | null {
  const { name, args } = toolCall(item);
  if (!TOOL_NAME.test(name) || !isRecord(args)) return null;
  const { title, html, height } = args;
  if (typeof html !== "string" || html.trim() === "") return null;
  return {
    title: typeof title === "string" && title.trim() ? title.trim() : "Visualization",
    html,
    maxHeight:
      typeof height === "number" && Number.isFinite(height)
        ? clampHtmlRenderHeight(height)
        : null,
  };
}

/** Whether a transcript row renders as an inline page rather than a tool
 *  card: an `html_render` call in flight (a placeholder) or one that
 *  succeeded with a complete page. A failed call stays a generic tool step
 *  so its error remains readable, and a call tied to an approval stays a
 *  `ToolCallCard` until it settles, since only that card carries the
 *  Allow/Deny footer. */
export function isInlineHtmlRender(item: ToolCallItem): boolean {
  if (item.status === "running") return item.approval_request_id == null && isHtmlRenderTool(item);
  return item.status === "done" && readHtmlRender(item) !== null;
}

export function clampHtmlRenderHeight(height: number): number {
  return Math.min(HTML_RENDER_MAX_HEIGHT, Math.max(HTML_RENDER_MIN_HEIGHT, Math.round(height)));
}

export interface HtmlRenderFonts {
  sans: string;
  mono: string;
}

export interface HtmlRenderTheme {
  scheme: "light" | "dark";
  variables: Record<string, string>;
}

// Categorical chart series after the brand accent, drawn from the theme's
// terminal palette so charts take on the theme's own hues.
const CHART_SLOTS = ["blue", "magenta", "green", "yellow", "cyan", "red"] as const;
const CHART_SERIES = 6;
// Below this RGB distance two series read as one color.
const MIN_SERIES_DISTANCE = 48;

function rgb(value: string): [number, number, number] | null {
  const hex = normalizeColor(value);
  if (!hex) return null;
  return [1, 3, 5].map((at) => parseInt(hex.slice(at, at + 2), 16)) as [number, number, number];
}

function chartSeries(theme: ThemeDefinition): string[] {
  const series = [theme.roles.brandAccent];
  const taken = [rgb(theme.roles.brandAccent)];
  for (const slot of CHART_SLOTS) {
    if (series.length === CHART_SERIES) break;
    const color = theme.ansi[slot];
    const value = rgb(color);
    const distinct = taken.every(
      (other) =>
        value === null ||
        other === null ||
        Math.hypot(value[0] - other[0], value[1] - other[1], value[2] - other[2]) >=
          MIN_SERIES_DISTANCE,
    );
    if (!distinct) continue;
    series.push(color);
    taken.push(value);
  }
  // A palette too narrow to fill every slot repeats its last hue rather than
  // leaving a variable undefined.
  while (series.length < CHART_SERIES) series.push(series[series.length - 1]!);
  return series;
}

/**
 * The variables an HTML render styles against. Names follow the app's own
 * tokens, except `--accent`, which is the theme's brand color here rather
 * than the app's neutral hover surface. `html_render`'s tool description in
 * `mcp_server.rs` documents this list for agents; keep the two in sync.
 */
export function htmlRenderTheme(theme: ThemeDefinition, fonts: HtmlRenderFonts): HtmlRenderTheme {
  const { roles, ansi } = theme;
  const accentForeground =
    contrastRatio(roles.brandAccent, roles.background) >=
    contrastRatio(roles.brandAccent, roles.foreground)
      ? roles.background
      : roles.foreground;
  const variables: Record<string, string> = {
    "--background": roles.background,
    "--foreground": roles.foreground,
    "--muted": roles.muted,
    "--muted-foreground": roles.mutedForeground,
    "--card": roles.card,
    "--card-foreground": roles.cardForeground,
    "--popover": roles.popover,
    "--popover-foreground": roles.popoverForeground,
    "--secondary": roles.secondary,
    "--secondary-foreground": roles.secondaryForeground,
    "--border": roles.border,
    "--input": roles.input,
    "--ring": roles.ring,
    "--primary": roles.primary,
    "--primary-foreground": roles.primaryForeground,
    "--accent": roles.brandAccent,
    "--accent-foreground": accentForeground,
    "--destructive": ansi.red,
    "--success": ansi.green,
    "--warning": ansi.yellow,
    "--info": ansi.blue,
    "--radius": theme.radius ?? "0.625rem",
    "--font-sans": fonts.sans,
    "--font-mono": fonts.mono,
  };
  chartSeries(theme).forEach((color, index) => {
    variables[`--chart-${index + 1}`] = color;
  });
  return { scheme: theme.scheme, variables };
}

// The bridge between a page and the app speaks the MCP Apps protocol
// (JSON-RPC over postMessage): https://github.com/modelcontextprotocol/ext-apps
const HOST_CONTEXT_CHANGED = "ui/notifications/host-context-changed";
const SIZE_CHANGED = "ui/notifications/size-changed";
const OPEN_LINK = "ui/open-link";

/** The notification the app posts into a mounted page when the theme changes. */
export function htmlRenderThemeMessage(theme: HtmlRenderTheme) {
  return {
    jsonrpc: "2.0",
    method: HOST_CONTEXT_CHANGED,
    params: { theme: theme.scheme, styles: { variables: theme.variables } },
  } as const;
}

/** The content height in a page's `size-changed` notification. */
export function readHtmlRenderContentHeight(data: unknown): number | undefined {
  if (!isRecord(data) || data.jsonrpc !== "2.0" || data.method !== SIZE_CHANGED) return undefined;
  const height = isRecord(data.params) ? data.params.height : undefined;
  return typeof height === "number" && Number.isFinite(height) && height > 0 ? height : undefined;
}

/** The http(s) URL in a page's `open-link` request. */
export function readHtmlRenderLinkRequest(data: unknown): string | undefined {
  if (!isRecord(data) || data.jsonrpc !== "2.0" || data.method !== OPEN_LINK) return undefined;
  const url = isRecord(data.params) ? data.params.url : undefined;
  return typeof url === "string" && /^https?:\/\//i.test(url) ? url : undefined;
}

// A scrollbar inside the reply reads as a box within the thread, so a page
// taller than its frame scrolls without one.
const BASE_CSS =
  "html{background:var(--background);color:var(--foreground);font-family:var(--font-sans);font-size:14px;line-height:1.5;-webkit-font-smoothing:antialiased;scrollbar-width:none}" +
  "html::-webkit-scrollbar{display:none}body{margin:0}code,kbd,pre,samp{font-family:var(--font-mono)}";

function themeCss(theme: HtmlRenderTheme): string {
  const declarations = Object.entries(theme.variables)
    .map(([name, value]) => `${name}:${value.replace(/[;{}<>]/g, "")};`)
    .join("");
  return `:root{color-scheme:${theme.scheme};${declarations}}${BASE_CSS}`;
}

// Runs first in <head>, so the page's own styles and scripts come after it.
// It rewrites its own <style> element on theme changes rather than setting
// inline properties, so a page's later `:root` rules still win. Clicked links
// never navigate the frame: the page asks the app to open http(s) ones, and
// other schemes do nothing. A `#fragment` link scrolls the page itself, since
// a srcdoc page's base URL is the app's, so following it natively would load
// the app into the frame. The page also
// reports its content height so the app can fit the frame to it.
const BOOTSTRAP_SCRIPT = `(function(){var s=document.getElementById("codemux-theme"),b=${JSON.stringify(BASE_CSS)};window.addEventListener("message",function(e){var d=e.data,p=d&&d.params;if(e.source!==window.parent||!d||d.jsonrpc!=="2.0"||d.method!==${JSON.stringify(HOST_CONTEXT_CHANGED)}||!p||!p.styles||!s)return;var v=p.styles.variables,c=":root{color-scheme:"+(p.theme==="light"?"light":"dark")+";";for(var k in v){if(/^--[a-z0-9-]+$/.test(k))c+=k+":"+String(v[k]).replace(/[;{}<>]/g,"")+";";}s.textContent=c+"}"+b;});document.addEventListener("click",function(e){var l=e.composedPath().find(function(t){return t&&t.matches&&t.matches("a[href]");}),u;if(!l)return;e.preventDefault();var r=l.getAttribute("href");if(r.charAt(0)==="#"){var i=r.slice(1),t;try{i=decodeURIComponent(i);}catch(x){}t=i?document.getElementById(i)||document.getElementsByName(i)[0]:document.documentElement;if(t)t.scrollIntoView();return;}try{u=new URL(r,document.baseURI);}catch(x){return;}if(/^https?:$/.test(u.protocol))window.parent.postMessage({jsonrpc:"2.0",method:${JSON.stringify(OPEN_LINK)},params:{url:u.href}},"*");},true);var h,z=function(){var r=document.documentElement,v=Math.ceil(r.scrollHeight>r.clientHeight?r.scrollHeight:r.getBoundingClientRect().height);if(v===h)return;h=v;window.parent.postMessage({jsonrpc:"2.0",method:${JSON.stringify(SIZE_CHANGED)},params:{height:v}},"*");};if(window.ResizeObserver){var o=new ResizeObserver(z);o.observe(document.documentElement);document.addEventListener("DOMContentLoaded",function(){if(document.body)o.observe(document.body);});}document.addEventListener("DOMContentLoaded",z);window.addEventListener("load",z);})();`;

// Comments and raw-text elements are blanked to the same length, so a
// `<head>` inside a script string or comment cannot receive the bootstrap.
function blankNonMarkup(html: string): string {
  return html.replace(
    /<!--[\s\S]*?(?:-->|$)|<(script|style|textarea|title|template|noscript)\b[\s\S]*?(?:<\/\1\s*>|$)/gi,
    (match) => " ".repeat(match.length),
  );
}

/** The page with the theme and bridge bootstrap inserted at the start of its head. */
export function buildHtmlRenderDocument(html: string, theme: HtmlRenderTheme): string {
  const markup =
    (/<meta\s[^>]*charset/i.test(html.slice(0, 4096)) ? "" : '<meta charset="utf-8">') +
    `<style id="codemux-theme">${themeCss(theme)}</style><script>${BOOTSTRAP_SCRIPT}</script>`;
  // A document's own `<html>` and `<head>` tags open it: only a doctype, the
  // `<html>` tag, whitespace, and comments (already blanked) can precede
  // them. Anywhere later, in body text or an attribute value, they are
  // content, not the document head.
  const scan = blankNonMarkup(html);
  const head = /^\s*(?:<!doctype[^>]*>\s*)?(?:<html(?:\s[^>]*)?>\s*)?<head(?:\s[^>]*)?>/i.exec(scan);
  if (head) {
    const at = head[0].length;
    return html.slice(0, at) + markup + html.slice(at);
  }
  const root = /^\s*(?:<!doctype[^>]*>\s*)?<html(?:\s[^>]*)?>/i.exec(scan);
  if (root) {
    const at = root[0].length;
    return `${html.slice(0, at)}<head>${markup}</head>${html.slice(at)}`;
  }
  const doctype = /^\s*<!doctype[^>]*>/i.exec(html);
  if (doctype) {
    const at = doctype[0].length;
    return `${html.slice(0, at)}<head>${markup}</head>${html.slice(at)}`;
  }
  return `<!doctype html><head>${markup}</head>${html}`;
}
