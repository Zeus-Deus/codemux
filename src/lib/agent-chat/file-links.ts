/** Workspace-safe parsing for source references emitted in agent Markdown. */

export const CHAT_FILE_LINK_TAG = "chat-file-link";

export interface ChatFileLinkMeta {
  filePath: string;
  basename: string;
  displayPath: string;
  line?: number;
  column?: number;
}

const WINDOWS_ABSOLUTE = /^[a-zA-Z]:[\\/]/;
const SCHEME = /^[a-zA-Z][a-zA-Z\d+.-]*:/;
const VERSION_LIKE = /^v?\d+(?:\.\d+)+(?:[-+][\w.-]+)?$/i;
const FILE_BASENAME = /^(?:[\w@+~. -]+\.)[a-zA-Z][a-zA-Z\d]{0,11}$/;
const SPECIAL_FILES = new Set([
  "Dockerfile",
  "Makefile",
  "Justfile",
  "Procfile",
  "Gemfile",
  "Rakefile",
  "AGENTS.md",
  "WORKFLOW.md",
]);

function slash(path: string): string {
  return path.replace(/\\/g, "/");
}

function normalizePath(path: string): string {
  const windowsPrefix = WINDOWS_ABSOLUTE.test(path) ? path.slice(0, 2) : "";
  const absolute = path.startsWith("/") || !!windowsPrefix;
  const source = slash(windowsPrefix ? path.slice(2) : path);
  const parts: string[] = [];
  for (const part of source.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (parts.length > 0) parts.pop();
      else if (!absolute) parts.push(part);
      continue;
    }
    parts.push(part);
  }
  const prefix = windowsPrefix ? `${windowsPrefix}/` : absolute ? "/" : "";
  return `${prefix}${parts.join("/")}` || (absolute ? prefix : ".");
}

function isAbsolute(path: string): boolean {
  return path.startsWith("/") || WINDOWS_ABSOLUTE.test(path);
}

function withinRoot(path: string, root: string): boolean {
  const candidate = normalizePath(path);
  const normalizedRoot = normalizePath(root).replace(/\/$/, "");
  const windows = WINDOWS_ABSOLUTE.test(candidate) || WINDOWS_ABSOLUTE.test(normalizedRoot);
  const left = windows ? candidate.toLowerCase() : candidate;
  const right = windows ? normalizedRoot.toLowerCase() : normalizedRoot;
  return left === right || left.startsWith(`${right}/`);
}

function basename(path: string): string {
  return slash(path).split("/").filter(Boolean).pop() ?? path;
}

function looksLikeFile(path: string): boolean {
  const name = basename(path);
  if (!name || VERSION_LIKE.test(name)) return false;
  if (SPECIAL_FILES.has(name) || name.startsWith(".env")) return true;
  return FILE_BASENAME.test(name);
}

function parseLocation(value: string): {
  path: string;
  line?: number;
  column?: number;
} {
  let path = value;
  let line: number | undefined;
  let column: number | undefined;

  const hash = /#L(\d+)(?::?C(\d+))?$/i.exec(path);
  if (hash) {
    line = Number(hash[1]);
    if (hash[2]) column = Number(hash[2]);
    path = path.slice(0, hash.index);
    return { path, line, column };
  }

  const suffix = /:(\d+)(?::(\d+))?$/.exec(path);
  if (suffix) {
    line = Number(suffix[1]);
    if (suffix[2]) column = Number(suffix[2]);
    path = path.slice(0, suffix.index);
  }
  return { path, line, column };
}

export interface ResolveChatFileLinkOptions {
  /**
   * Whether the path portion may contain unencoded whitespace. Href sources
   * keep this on (a legitimate `%20` decodes into a space); inline-code spans
   * turn it off, because prose like `cargo check --manifest-path
   * src-tauri/Cargo.toml` or `cat src/foo.ts` ends in a file-like segment
   * without being a file reference. A plain inline-code token is preferable
   * to a confident chip that opens the wrong file. Defaults to `true`.
   */
  allowSpaces?: boolean;
}

export function resolveChatFileLink(
  candidate: string,
  cwd?: string | null,
  options: ResolveChatFileLinkOptions = {},
): ChatFileLinkMeta | null {
  if (!cwd || !candidate || candidate.includes("\n") || candidate.includes("\0")) {
    return null;
  }
  let decoded = candidate.trim();
  try {
    decoded = decodeURIComponent(decoded);
  } catch {
    // Keep the literal form when malformed percent escapes arrive mid-stream.
  }
  if (!decoded || decoded.startsWith("#")) return null;
  if (/^file:\/\//i.test(decoded)) decoded = decoded.replace(/^file:\/\//i, "");
  else if (SCHEME.test(decoded) && !WINDOWS_ABSOLUTE.test(decoded)) return null;
  if (decoded.includes("?") || decoded.includes("`")) return null;

  const location = parseLocation(decoded);
  if (options.allowSpaces === false && /\s/.test(location.path)) return null;
  if (!looksLikeFile(location.path)) return null;
  const root = normalizePath(cwd);
  const resolved = isAbsolute(location.path)
    ? normalizePath(location.path)
    : normalizePath(`${root}/${location.path}`);
  if (!withinRoot(resolved, root)) return null;

  const relative = resolved === root ? basename(resolved) : resolved.slice(root.length + 1);
  return {
    filePath: resolved,
    basename: basename(resolved),
    displayPath: relative || basename(resolved),
    ...(location.line ? { line: location.line } : {}),
    ...(location.column ? { column: location.column } : {}),
  };
}

interface HastNode {
  type: string;
  tagName?: string;
  properties?: Record<string, unknown>;
  children?: HastNode[];
}

function looksLikeSourceHref(candidate: string): boolean {
  let decoded = candidate.trim();
  try {
    decoded = decodeURIComponent(decoded);
  } catch {
    // The resolver will preserve malformed escapes as literal text too.
  }
  if (!decoded || decoded.startsWith("#") || decoded.includes("?")) return false;
  if (/^file:\/\//i.test(decoded)) decoded = decoded.replace(/^file:\/\//i, "");
  else if (SCHEME.test(decoded) && !WINDOWS_ABSOLUTE.test(decoded)) return false;
  return looksLikeFile(parseLocation(decoded).path);
}

function visitFileLinks(node: HastNode) {
  if (node.type === "element" && node.tagName === "a") {
    const href = node.properties?.href;
    if (typeof href === "string" && looksLikeSourceHref(href)) {
      const { href: _href, ...properties } = node.properties ?? {};
      void _href;
      node.tagName = CHAT_FILE_LINK_TAG;
      node.properties = {
        ...properties,
        sourceHref: href,
      };
    }
  }
  for (const child of node.children ?? []) visitFileLinks(child);
}

/** Stable rehype transform. Resolution stays in the React component so
 * Streamdown's processor cache cannot capture the first workspace root. */
export function rehypeChatFileLinks() {
  return (tree: HastNode) => visitFileLinks(tree);
}
