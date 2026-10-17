/**
 * Evaluate a disk plugin's plain-ESM source: import allowlist, specifier
 * rewrite to live shim blobs, blob `import()` raced against a deadline.
 */
import { rewriteSpecifiers, unsupportedImports } from "./import-lexer";
import type { CodemuxPlugin } from "./plugin";
import { installPluginSdk, sdkImportMap } from "@/sdk/runtime";

/** A module whose evaluation never settles (top-level await on something
 *  that never resolves) fails on its own row instead of hanging the scan. */
export const IMPORT_TIMEOUT_MS = 10_000;

export async function evaluatePluginSource(
  source: string,
  origin: string,
  timeoutMs = IMPORT_TIMEOUT_MS,
): Promise<CodemuxPlugin> {
  installPluginSdk();
  const map = sdkImportMap();
  const unsupported = unsupportedImports(source, (spec) => spec in map);
  if (unsupported.length > 0) {
    throw new Error(
      `unsupported import${unsupported.length > 1 ? "s" : ""}: ${unsupported.join(", ")}. ` +
        "Disk plugins may only import @codemux/sdk and react.",
    );
  }
  const url = URL.createObjectURL(
    new Blob([rewriteSpecifiers(source, map)], { type: "text/javascript" }),
  );
  let deadline: ReturnType<typeof setTimeout> | undefined;
  let mod: { default?: CodemuxPlugin };
  try {
    mod = await Promise.race([
      import(/* @vite-ignore */ url) as Promise<{ default?: CodemuxPlugin }>,
      new Promise<never>((_, reject) => {
        deadline = setTimeout(
          () =>
            reject(
              new Error(
                `import timed out after ${timeoutMs / 1000}s: module evaluation never settled`,
              ),
            ),
          timeoutMs,
        );
      }),
    ]);
  } finally {
    clearTimeout(deadline);
    URL.revokeObjectURL(url);
  }
  const plugin = mod.default;
  if (!plugin || typeof plugin !== "object" || typeof plugin.register !== "function" || !plugin.id) {
    throw new Error(`${origin}/plugin.js has no valid default export ({ id, register })`);
  }
  return plugin;
}
