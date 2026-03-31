import '@testing-library/jest-dom/vitest';

// Radix UI popper components require ResizeObserver which jsdom lacks
if (typeof globalThis.ResizeObserver === 'undefined') {
  globalThis.ResizeObserver = class ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
}
