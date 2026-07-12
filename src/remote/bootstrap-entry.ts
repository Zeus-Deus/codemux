/**
 * Entry module for the web-remote runtime, imported by `main.tsx` behind
 * the "plain browser, no Tauri webview" guard (and, in dev, `?remote=1`).
 * `main.tsx` awaits {@link bootstrapRemote} before mounting React, so
 * pairing + shim install + first connect all complete before any
 * component calls `invoke()`.
 *
 * Re-exports the entry function rather than running it at module top
 * level: a surviving production chunk targets es2020, which has no
 * top-level-await support (the dev mock only gets away with top-level
 * `await import(...)` because `import.meta.env.DEV` strips that branch
 * out of the prod bundle entirely). `main.tsx` performs the `await`
 * inside an async function instead.
 */
export { bootstrapRemote } from "./bootstrap";
