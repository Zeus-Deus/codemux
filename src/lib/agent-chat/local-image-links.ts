/**
 * Recognise agent-authored links to local image files and turn them into a
 * dedicated chat element. Agents commonly hand visual proof back as a normal
 * Markdown link (`[Screenshot](/absolute/path.png)`) rather than image syntax;
 * a browser anchor cannot navigate to that filesystem path, so ChatMarkdown
 * upgrades both forms to the same preview component.
 */

export const CHAT_LOCAL_IMAGE_TAG = "chat-local-image";

const SUPPORTED_IMAGE_EXTENSION = /\.(?:png|jpe?g|gif|webp)$/i;

interface HastNode {
  type: string;
  tagName?: string;
  value?: string;
  properties?: Record<string, unknown>;
  children?: HastNode[];
}

function decodePath(path: string): string {
  try {
    return decodeURI(path);
  } catch {
    return path;
  }
}

/**
 * Return the native absolute path represented by a Markdown destination, or
 * null when it is not a supported local image. POSIX paths, Windows drive
 * paths, and local `file://` URLs are accepted; relative paths are deliberately
 * left alone because a chat message has no stable document directory.
 */
export function localImagePath(destination: unknown): string | null {
  if (typeof destination !== "string" || !destination) return null;

  let path = destination;
  if (/^file:/i.test(path)) {
    try {
      const url = new URL(path);
      if (url.protocol !== "file:" || (url.hostname && url.hostname !== "localhost")) {
        return null;
      }
      path = decodeURIComponent(url.pathname);
      // file:///C:/foo.png is the standard Windows file-URL spelling.
      if (/^\/[a-zA-Z]:\//.test(path)) path = path.slice(1);
    } catch {
      return null;
    }
  } else {
    path = decodePath(path);
  }

  const absolute = path.startsWith("/") || /^[a-zA-Z]:[\\/]/.test(path);
  return absolute && SUPPORTED_IMAGE_EXTENSION.test(path) ? path : null;
}

function textContent(node: HastNode): string {
  if (node.type === "text") return node.value ?? "";
  return (node.children ?? []).map(textContent).join("");
}

function upgradeNode(node: HastNode) {
  if (node.type === "element" && node.tagName === "a") {
    const path = localImagePath(node.properties?.href);
    if (path) {
      node.tagName = CHAT_LOCAL_IMAGE_TAG;
      node.properties = {
        path,
        caption: textContent(node).trim(),
        sourceSyntax: "link",
      };
    }
  } else if (node.type === "element" && node.tagName === "img") {
    const path = localImagePath(node.properties?.src);
    if (path) {
      const caption =
        typeof node.properties?.alt === "string" ? node.properties.alt.trim() : "";
      node.tagName = CHAT_LOCAL_IMAGE_TAG;
      node.properties = { path, caption, sourceSyntax: "image" };
      node.children = [];
    }
  }

  for (const child of node.children ?? []) upgradeNode(child);
}

/** Rehype plugin entry point. */
export function rehypeLocalImageLinks() {
  return (tree: HastNode) => upgradeNode(tree);
}
