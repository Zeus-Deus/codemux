import '@testing-library/jest-dom/vitest';
import { vi } from 'vitest';

// Radix UI popper components require ResizeObserver which jsdom lacks
if (typeof globalThis.ResizeObserver === 'undefined') {
  globalThis.ResizeObserver = class ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
}

// Tauri's `getCurrentWindow()` reads `window.__TAURI_INTERNALS__.metadata`
// which is injected by the Tauri runtime at app start. Under jsdom there is
// no Tauri runtime, so any test that renders a component which calls
// `getCurrentWindow()` (e.g. `<WindowControls />` from window-chrome.tsx)
// would otherwise crash with "Cannot read properties of undefined (reading
// 'metadata')". We mock the window module globally so every test gets a
// no-op stub by default — individual tests can override this with their
// own `vi.mock(...)` if they need specific behavior.
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    isMaximized: () => Promise.resolve(false),
    onResized: (_handler: () => void) => Promise.resolve(() => {}),
    minimize: () => Promise.resolve(),
    maximize: () => Promise.resolve(),
    toggleMaximize: () => Promise.resolve(),
    unmaximize: () => Promise.resolve(),
    close: () => Promise.resolve(),
    show: () => Promise.resolve(),
    setFocus: () => Promise.resolve(),
    unminimize: () => Promise.resolve(),
    requestUserAttention: () => Promise.resolve(),
  }),
}));
