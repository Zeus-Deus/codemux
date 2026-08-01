/**
 * Rehype transform for compact, recognisable external links in chat.
 *
 * t3code's link treatment is deliberately generic: the markdown author still
 * controls the label ("PR #235", "docs", a bare URL, …), while every absolute
 * http(s) destination receives the destination site's favicon. This transform
 * applies the same idea without replacing Streamdown's anchor renderer, so its
 * existing safe-link confirmation remains intact.
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

function leadingTextLength(text: string): number {
  const protocol = /^(?:https?:\/\/)/i.exec(text)?.[0];
  if (protocol) return protocol.length;
  return Math.min(text.length, 1);
}

function faviconNode(host: string): HastNode {
  return {
    type: "element",
    tagName: CHAT_LINK_FAVICON_TAG,
    properties: { host },
    children: [],
  };
}

function decorateExternalLink(node: HastNode, host: string) {
  const [firstChild, ...remainingChildren] = node.children ?? [];
  const leadingChildren = [faviconNode(host)];

  if (firstChild?.type === "text" && typeof firstChild.value === "string") {
    const leadingLength = leadingTextLength(firstChild.value);
    leadingChildren.push({
      type: "text",
      value: firstChild.value.slice(0, leadingLength),
    });

    const rest = firstChild.value.slice(leadingLength);
    node.children = [
      {
        type: "element",
        tagName: "span",
        properties: { className: [CHAT_LINK_LEADING_CLASS] },
        children: leadingChildren,
      },
      ...(rest ? [{ type: "text", value: rest }] : []),
      ...remainingChildren,
    ];
    return;
  }

  if (firstChild) leadingChildren.push(firstChild);
  node.children = [
    {
      type: "element",
      tagName: "span",
      properties: { className: [CHAT_LINK_LEADING_CLASS] },
      children: leadingChildren,
    },
    ...remainingChildren,
  ];
}

function visit(node: HastNode) {
  if (node.type === "element" && node.tagName === "a") {
    const host = externalWebLinkHost(node.properties?.href);
    if (host) decorateExternalLink(node, host);
  }

  for (const child of node.children ?? []) visit(child);
}

/** Rehype plugin entry point. */
export function rehypeRichExternalLinks() {
  return (tree: HastNode) => visit(tree);
}
