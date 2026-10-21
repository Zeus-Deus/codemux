export const CHAT_LINK_PREFIX = "codemux-chat://v1/";

export function chatLinkId(href: string | undefined): string | null {
  if (!href?.startsWith(CHAT_LINK_PREFIX)) return null;
  try {
    const id = decodeURIComponent(href.slice(CHAT_LINK_PREFIX.length));
    return id && id.length <= 256 && !/[\s/?#\u0000-\u001f]/.test(id) ? id : null;
  } catch { return null; }
}
