/**
 * Rehype transform for compact, recognisable external links in chat.
 *
 * The treatment is deliberately generic: the markdown author still controls
 * the label ("PR #235", "docs", a bare URL, …), while absolute http(s)
 * destinations pick up an icon. Only a short list of known hosts uses the
 * destination site's favicon; other hosts use a neutral icon. The transform
 * decorates the anchor's children instead of replacing Streamdown's renderer,
 * so its existing safe-link confirmation remains intact.
 */

export const CHAT_LINK_FAVICON_TAG = "chat-link-favicon";
export const CHAT_LINK_LEADING_CLASS = "chat-markdown-link-leading";

interface HastNode {
  type: string;
  tagName?: string;
  value?: string;
  properties?: Record<string, unknown>;
  children?: HastNode[];
}

export function externalWebLinkHost(href: unknown): string | null {
  if (typeof href !== "string") return null;
  try {
    const url = new URL(href);
    if (url.protocol !== "http:" && url.protocol !== "https:") return null;
    return url.hostname || null;
  } catch {
    return null;
  }
}

/** A favicon lookup exposes the host of a chat link to a third party.
 * An arbitrary dotted name may resolve only on a private network, so only
 * these exact, well-known names are sent. Other links use a neutral icon. */
const TRUSTED_FAVICON_HOSTS = new Set([
  "bitbucket.org",
  "crates.io",
  "developer.mozilla.org",
  "docs.codemux.org",
  "docs.rs",
  "github.com",
  "gitlab.com",
  "npmjs.com",
  "platform.openai.com",
  "stackoverflow.com",
]);

export function trustedFaviconHost(host: unknown): string | null {
  if (typeof host !== "string") return null;
  const normalized = host.toLowerCase().replace(/\.$/, "");
  return TRUSTED_FAVICON_HOSTS.has(normalized) ? normalized : null;
}

function leadingTextLength(text: string): number {
  const protocol = /^(?:https?:\/\/)/i.exec(text)?.[0];
  if (protocol) return protocol.length;
  // One whole code point, so an astral first character (an emoji label) is
  // never split into lone surrogates.
  const [firstCodePoint] = text;
  return firstCodePoint?.length ?? 0;
}

function faviconNode(host: string, href: string): HastNode {
  return {
    type: "element",
    tagName: CHAT_LINK_FAVICON_TAG,
    properties: { host, href },
    children: [],
  };
}

function leadingSpan(children: HastNode[]): HastNode {
  return {
    type: "element",
    tagName: "span",
    properties: { className: [CHAT_LINK_LEADING_CLASS] },
    children,
  };
}

/**
 * Elements we never reach inside: they either draw their own box (an inline
 * code pill) or hold no text, so an icon spliced into them would land in the
 * wrong place.
 */
const OPAQUE_LABEL_TAGS = new Set([
  "code",
  "kbd",
  "samp",
  "img",
  "picture",
  "svg",
  "video",
]);

/**
 * Glue the favicon to the very start of the link's text inside a nowrap span
 * so the icon can never wrap onto a line of its own.
 *
 * Only the protocol (bare URLs) or the first character (labelled links) goes
 * into that span. When the label starts with an element — `[**a bold
 * label**](…)` — descend into it and split its first text node rather than
 * wrapping the whole element, otherwise a long styled label would be
 * unwrappable and would overflow a narrow pane. Returns false when there is no
 * text to glue to.
 */
function attachLeadingFavicon(container: HastNode, favicon: HastNode): boolean {
  const children = container.children;
  const firstChild = children?.[0];
  if (!children || !firstChild) return false;

  if (firstChild.type === "text" && typeof firstChild.value === "string") {
    if (!firstChild.value) return false;
    const leadingLength = leadingTextLength(firstChild.value);
    const rest = firstChild.value.slice(leadingLength);
    children.splice(
      0,
      1,
      leadingSpan([favicon, { type: "text", value: firstChild.value.slice(0, leadingLength) }]),
      ...(rest ? [{ type: "text", value: rest } satisfies HastNode] : []),
    );
    return true;
  }

  if (firstChild.type === "element" && !OPAQUE_LABEL_TAGS.has(firstChild.tagName ?? "")) {
    return attachLeadingFavicon(firstChild, favicon);
  }

  return false;
}

function decorateExternalLink(node: HastNode, host: string, href: string) {
  const favicon = faviconNode(host, href);
  if (attachLeadingFavicon(node, favicon)) return;

  // No splittable text (an image or inline-code label). Keep the icon glued to
  // that whole first child instead — such labels are self-contained boxes, so
  // nothing is made unwrappable that was not already.
  const [firstChild, ...remainingChildren] = node.children ?? [];
  node.children = firstChild
    ? [leadingSpan([favicon, firstChild]), ...remainingChildren]
    : [favicon];
}

function visit(node: HastNode) {
  if (node.type === "element" && node.tagName === "a") {
    const href = node.properties?.href;
    const host = externalWebLinkHost(href);
    if (host) {
      if (typeof node.properties?.title !== "string") {
        node.properties = {
          ...node.properties,
          title: node.properties?.href,
        };
      }
      decorateExternalLink(node, host, String(href));
    }
  }

  for (const child of node.children ?? []) visit(child);
}

/** Rehype plugin entry point. */
export function rehypeRichExternalLinks() {
  return (tree: HastNode) => visit(tree);
}
