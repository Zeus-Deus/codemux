/**
 * Rehype transform for compact, recognisable external links in chat.
 *
 * The treatment is deliberately generic: the markdown author still controls
 * the label ("PR #235", "docs", a bare URL, …), while every absolute http(s)
 * destination picks up the destination site's favicon — no per-site parsing,
 * no preview cards. The transform decorates the anchor's children instead of
 * replacing Streamdown's anchor renderer, so its existing safe-link
 * confirmation remains intact.
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

/**
 * Suffixes reserved for names that only resolve inside a private network.
 * A favicon lookup for one of these would leak an internal hostname to a
 * third-party service and could never succeed anyway.
 */
const INTERNAL_HOST_SUFFIXES = [
  ".local",
  ".localhost",
  ".internal",
  ".lan",
  ".home",
  ".corp",
];

function ipv4Octets(host: string): number[] | null {
  const parts = host.split(".");
  if (parts.length !== 4) return null;
  const octets: number[] = [];
  for (const part of parts) {
    if (!/^\d{1,3}$/.test(part)) return null;
    const value = Number(part);
    if (value > 255) return null;
    octets.push(value);
  }
  return octets;
}

function isPrivateIpv4([a, b]: number[]): boolean {
  if (a === 10 || a === 127 || a === 0) return true;
  if (a === 172 && b >= 16 && b <= 31) return true;
  if (a === 192 && b === 168) return true;
  if (a === 169 && b === 254) return true;
  return false;
}

function isPrivateIpv6(host: string): boolean {
  const address = host.replace(/^\[|\]$/g, "").split("%")[0]!.toLowerCase();
  if (address === "::1" || address === "::") return true;
  // fc00::/7 (unique local) and fe80::/10 (link local).
  const [first] = address.split(":");
  if (!first) return false;
  const block = Number.parseInt(first.padStart(4, "0"), 16);
  if (Number.isNaN(block)) return false;
  if (block >= 0xfc00 && block <= 0xfdff) return true;
  if (block >= 0xfe80 && block <= 0xfebf) return true;
  return false;
}

/**
 * Whether a hostname is public enough to look up on a third-party favicon
 * service. Every favicon request tells that service which host a chat link
 * points at, so intranet names and private/reserved IP literals are never
 * sent — those links render the neutral globe instead.
 */
export function isPublicWebHost(host: unknown): boolean {
  if (typeof host !== "string") return false;
  const normalized = host.trim().toLowerCase().replace(/\.+$/, "");
  if (!normalized) return false;

  if (normalized.startsWith("[") || normalized.includes(":")) {
    return !isPrivateIpv6(normalized);
  }

  const octets = ipv4Octets(normalized);
  if (octets) return !isPrivateIpv4(octets);

  // Single-label names (`localhost`, `intranet`) only resolve on a local
  // network or through a search domain — never publicly.
  if (!normalized.includes(".")) return false;

  return !INTERNAL_HOST_SUFFIXES.some((suffix) => normalized.endsWith(suffix));
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

function decorateExternalLink(node: HastNode, host: string) {
  const favicon = faviconNode(host);
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
    const host = externalWebLinkHost(node.properties?.href);
    if (host) decorateExternalLink(node, host);
  }

  for (const child of node.children ?? []) visit(child);
}

/** Rehype plugin entry point. */
export function rehypeRichExternalLinks() {
  return (tree: HastNode) => visit(tree);
}
