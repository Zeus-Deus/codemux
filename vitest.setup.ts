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

// cmdk's CommandItem highlight scrolls into view on open; jsdom has
// no scrollIntoView. Stubbing avoids a hard crash when Popover+Command
// mounts in tests.
if (typeof Element !== 'undefined' && !Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = function scrollIntoViewStub() {};
}

// Radix Popover/Dialog use the pointer-capture APIs. jsdom doesn't
// implement them, which crashes userEvent-driven clicks inside
// popovers. Polyfill with no-ops.
interface JsdomElementShim {
  hasPointerCapture?: (pointerId: number) => boolean;
  releasePointerCapture?: (pointerId: number) => void;
  setPointerCapture?: (pointerId: number) => void;
}
const proto = typeof Element !== 'undefined'
  ? (Element.prototype as unknown as JsdomElementShim)
  : null;
if (proto && !proto.hasPointerCapture) {
  proto.hasPointerCapture = () => false;
  proto.releasePointerCapture = () => {};
  proto.setPointerCapture = () => {};
}

// jsdom (as of the version this project pins) does not implement
// PointerEvent. Radix's PopoverTrigger listens for pointerdown, so
// without this polyfill userEvent-driven clicks do not reach Radix's
// open handler and the popover never mounts. Delegate construction to
// MouseEvent — Radix only reads `pointerId`, `button`, and
// `ctrlKey`/`shiftKey`, all of which MouseEvent provides.
if (typeof window !== 'undefined' && typeof window.PointerEvent === 'undefined') {
  class PolyfillPointerEvent extends MouseEvent {
    readonly pointerId: number;
    readonly pointerType: string;
    readonly isPrimary: boolean;
    constructor(type: string, init: PointerEventInit = {}) {
      super(type, init);
      this.pointerId = init.pointerId ?? 0;
      this.pointerType = init.pointerType ?? '';
      this.isPrimary = init.isPrimary ?? false;
    }
  }
  (window as unknown as { PointerEvent: typeof PointerEvent }).PointerEvent =
    PolyfillPointerEvent as unknown as typeof PointerEvent;
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

// `convertFileSrc()` from `@tauri-apps/api/core` reads
// `window.__TAURI_INTERNALS__.convertFileSrc`, which only exists under the
// Tauri runtime. Components that render local-file thumbnails (e.g. image
// attachment chips) call it during render, so without a stub they crash
// under jsdom. Override only `convertFileSrc` and keep the module's other
// exports (notably `invoke`, used by the real command wrapper) intact —
// tests that assert on the resolved URL still override this themselves.
vi.mock('@tauri-apps/api/core', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@tauri-apps/api/core')>();
  return { ...actual, convertFileSrc: (filePath: string) => filePath };
});
