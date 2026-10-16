/**
 * Runtime SDK injection for disk plugins. Bundled plugins resolve
 * `@codemux/sdk` through the Vite alias; disk plugins import the same
 * specifier and get the same object: the loader rewrites bare specifiers to
 * tiny shim modules that re-export the live namespaces installed here. React
 * is the app's own singleton; a second React would break hooks.
 */
import * as React from "react";
import * as jsxDevRuntime from "react/jsx-dev-runtime";
import * as jsxRuntime from "react/jsx-runtime";
import * as sdk from "./index";

// Resolved LAZILY, never as a module-scope literal: this module sits in an
// import cycle (`sdk/index` → contrib → engine → `sdk/runtime`), so a
// module-scope object would be built before `sdk/index` finished evaluating
// and hold `undefined` in the production bundle (the bug Hermes hit).
function namespaces() {
  return {
    __CODEMUX_SDK__: sdk,
    __CODEMUX_REACT__: React,
    __CODEMUX_REACT_JSX__: jsxRuntime,
    __CODEMUX_REACT_JSX_DEV__: jsxDevRuntime,
  };
}

type GlobalKey = keyof ReturnType<typeof namespaces>;

export function installPluginSdk(): void {
  Object.assign(globalThis, namespaces());
}

/** A shim ESM blob re-exporting a global namespace. Names come from the
 *  namespace itself, so the list can't drift. */
function shimUrl(key: GlobalKey): string {
  const names = Object.keys(namespaces()[key]).filter(
    (name) => name !== "default" && /^[A-Za-z_$][\w$]*$/.test(name),
  );
  const source =
    `const m = globalThis.${key};\n` +
    `export default m.default ?? m;\n` +
    (names.length ? `export const { ${names.join(", ")} } = m;\n` : "");
  return URL.createObjectURL(new Blob([source], { type: "text/javascript" }));
}

let cached: Record<string, string> | null = null;

/** Allowed specifier → shim URL. */
export function sdkImportMap(): Record<string, string> {
  cached ??= {
    "@codemux/sdk": shimUrl("__CODEMUX_SDK__"),
    "react/jsx-dev-runtime": shimUrl("__CODEMUX_REACT_JSX_DEV__"),
    "react/jsx-runtime": shimUrl("__CODEMUX_REACT_JSX__"),
    react: shimUrl("__CODEMUX_REACT__"),
  };
  return cached;
}

export const ALLOWED_SPECIFIERS = [
  "@codemux/sdk",
  "react",
  "react/jsx-runtime",
  "react/jsx-dev-runtime",
] as const;
