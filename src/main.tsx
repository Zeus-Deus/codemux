import React from "react";
import ReactDOM from "react-dom/client";
import { TooltipProvider } from "@/components/ui/tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import App from "./App";
import "./globals.css";

// Single QueryClient for the whole app. Defaults are tuned for the
// Review-tab use case where we want fresh-ish data on focus + a
// reasonable refetch interval, without thrashing `gh` on quiet windows.
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 10_000,
      refetchOnWindowFocus: true,
      retry: 1,
    },
  },
});

// Dev only: the mock's pull-request toast triggers need to make the
// shared overview query refetch, and the query client is the only thing
// that can do that. `import.meta.env.DEV` is statically false in
// production, so this block is not in the shipped bundle.
if (import.meta.env.DEV) {
  (window as unknown as { __codemuxQueryClient: QueryClient }).__codemuxQueryClient =
    queryClient;
}

document.documentElement.classList.add("dark");

function dismissSplash() {
  const splash = document.getElementById("splash");
  if (!splash) return;
  splash.classList.add("fade-out");
  splash.addEventListener("transitionend", () => splash.remove());
}

function mountApp() {
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <QueryClientProvider client={queryClient}>
        <TooltipProvider>
          <App />
        </TooltipProvider>
      </QueryClientProvider>
    </React.StrictMode>,
  );
  // Dismiss after React has painted its first frame
  requestAnimationFrame(() => requestAnimationFrame(dismissSplash));
}

// Runtime shim selection. The UI talks to its backend exclusively through
// `window.__TAURI_INTERNALS__`; three runtimes provide it:
//   1. The Tauri desktop WebView injects the REAL internals before this
//      script runs — nothing to install, real IPC.
//   2. `npm run dev` in a plain browser installs the dev mock
//      (`src/dev/tauri-mock.ts`): in-process fixtures, no backend.
//   3. A plain browser served by the desktop app's web-remote server
//      installs the WebSocket shim (`src/remote/`): real IPC over a socket
//      to the running desktop instance, gated on device pairing.
// Both shims stay dormant under the desktop WebView (its
// `__TAURI_INTERNALS__` is already present), so real-IPC behavior is
// byte-identical to today. `import.meta.env.DEV` is statically `false` in
// production, so Rollup drops the dev-mock import from the prod bundle
// entirely (`grep -r "tauri-mock" dist/` stays empty). The awaits run
// inside this async function — rather than at the module top level —
// because a surviving production chunk targets es2020, which has no
// top-level-await support; the effect is the same: the chosen shim is
// installed (and, for the remote shim, pairing has succeeded and the
// socket is live) before React mounts and any component calls `invoke()`.
async function installRuntimeShim(): Promise<void> {
  if (import.meta.env.DEV && new URLSearchParams(location.search).has("remote")) {
    // Dev-only: exercise the real WebSocket shim against a running server
    // (`npm run dev` + `?remote=1`) instead of the in-process mock.
    const { bootstrapRemote } = await import("./remote/bootstrap-entry");
    await bootstrapRemote();
    return;
  }
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
    await import("./dev/tauri-mock");
    return;
  }
  if (!import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
    const { bootstrapRemote } = await import("./remote/bootstrap-entry");
    await bootstrapRemote();
  }
}

void installRuntimeShim().then(mountApp);
