import React from "react";
import ReactDOM from "react-dom/client";
import { TooltipProvider } from "@/components/ui/tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import App from "./App";
import "./globals.css";

// Dev-only Tauri runtime shim. Two guards, both required:
//   - `import.meta.env.DEV` is statically `false` in production, so
//     Rollup drops this entire branch from the prod bundle (verified by
//     `grep -r "tauri-mock" dist/` being empty after `npm run build`).
//   - The `__TAURI_INTERNALS__` check keeps the mock dormant under
//     `npm run tauri:dev`, where the real WebView already injected it —
//     so real-IPC behavior is byte-identical to today.
// The top-level await guarantees the shim is installed before React
// mounts and any component calls `invoke()`.
if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
  await import("./dev/tauri-mock");
}

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

document.documentElement.classList.add("dark");

function dismissSplash() {
  const splash = document.getElementById("splash");
  if (!splash) return;
  splash.classList.add("fade-out");
  splash.addEventListener("transitionend", () => splash.remove());
}

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
