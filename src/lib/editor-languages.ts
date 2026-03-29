import type { Extension } from "@codemirror/state";

type LanguageLoader = () => Promise<Extension>;

const LANG_MAP: Record<string, LanguageLoader> = {
  ts: () => import("@codemirror/lang-javascript").then((m) => m.javascript({ typescript: true })),
  tsx: () => import("@codemirror/lang-javascript").then((m) => m.javascript({ typescript: true, jsx: true })),
  js: () => import("@codemirror/lang-javascript").then((m) => m.javascript()),
  mjs: () => import("@codemirror/lang-javascript").then((m) => m.javascript()),
  cjs: () => import("@codemirror/lang-javascript").then((m) => m.javascript()),
  jsx: () => import("@codemirror/lang-javascript").then((m) => m.javascript({ jsx: true })),
  rs: () => import("@codemirror/lang-rust").then((m) => m.rust()),
  json: () => import("@codemirror/lang-json").then((m) => m.json()),
  jsonc: () => import("@codemirror/lang-json").then((m) => m.json()),
  css: () => import("@codemirror/lang-css").then((m) => m.css()),
  scss: () => import("@codemirror/lang-css").then((m) => m.css()),
  less: () => import("@codemirror/lang-css").then((m) => m.css()),
  html: () => import("@codemirror/lang-html").then((m) => m.html()),
  htm: () => import("@codemirror/lang-html").then((m) => m.html()),
  svelte: () => import("@codemirror/lang-html").then((m) => m.html()),
  vue: () => import("@codemirror/lang-html").then((m) => m.html()),
  md: () => import("@codemirror/lang-markdown").then((m) => m.markdown()),
  mdx: () => import("@codemirror/lang-markdown").then((m) => m.markdown()),
  py: () => import("@codemirror/lang-python").then((m) => m.python()),
  go: () => import("@codemirror/lang-go").then((m) => m.go()),
  yaml: () => import("@codemirror/lang-yaml").then((m) => m.yaml()),
  yml: () => import("@codemirror/lang-yaml").then((m) => m.yaml()),
};

export async function loadLanguage(filename: string): Promise<Extension | null> {
  const ext = filename.split(".").pop()?.toLowerCase();
  if (!ext) return null;
  const loader = LANG_MAP[ext];
  if (!loader) return null;
  try {
    return await loader();
  } catch {
    return null;
  }
}

const BINARY_EXTENSIONS = new Set([
  "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "avif", "svg",
  "woff", "woff2", "ttf", "eot", "otf",
  "zip", "tar", "gz", "bz2", "xz", "7z", "rar",
  "exe", "dll", "so", "dylib", "o", "a",
  "wasm", "pdf", "mp3", "mp4", "wav", "ogg", "webm",
]);

export function isBinaryExtension(filename: string): boolean {
  const ext = filename.split(".").pop()?.toLowerCase();
  return ext != null && BINARY_EXTENSIONS.has(ext);
}
