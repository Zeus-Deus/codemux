import { afterEach, describe, expect, it, vi } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { randomUUID } from "./uuid";

const UUID_V4 =
  /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

/** Swap the ambient `crypto` for `value`, returning a restore fn. The
 *  real `globalThis.crypto` is a non-writable accessor in some runtimes,
 *  so we redefine the property rather than assign to it. */
function withCrypto(value: unknown): () => void {
  const original = Object.getOwnPropertyDescriptor(globalThis, "crypto");
  Object.defineProperty(globalThis, "crypto", {
    configurable: true,
    writable: true,
    value,
  });
  return () => {
    if (original) Object.defineProperty(globalThis, "crypto", original);
    else delete (globalThis as { crypto?: unknown }).crypto;
  };
}

describe("randomUUID", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("returns the native value when crypto.randomUUID is available", () => {
    const native = "11111111-2222-4333-8444-555555555555";
    const restore = withCrypto({
      ...globalThis.crypto,
      randomUUID: () => native,
      getRandomValues: globalThis.crypto.getRandomValues?.bind(globalThis.crypto),
    });
    try {
      expect(randomUUID()).toBe(native);
    } finally {
      restore();
    }
  });

  // The remote web client is served over plain http, which is not a
  // secure context — `crypto.randomUUID` simply isn't there.
  it("mints a canonical v4 uuid when crypto.randomUUID is unavailable", () => {
    const restore = withCrypto({
      getRandomValues: (bytes: Uint8Array) => {
        for (let i = 0; i < bytes.length; i++) {
          bytes[i] = Math.floor(Math.random() * 256);
        }
        return bytes;
      },
    });
    try {
      const a = randomUUID();
      const b = randomUUID();
      expect(a).toMatch(UUID_V4);
      expect(b).toMatch(UUID_V4);
      expect(a).not.toBe(b);
    } finally {
      restore();
    }
  });

  it("still mints a canonical v4 uuid without getRandomValues either", () => {
    const restore = withCrypto({});
    try {
      const a = randomUUID();
      const b = randomUUID();
      expect(a).toMatch(UUID_V4);
      expect(b).toMatch(UUID_V4);
      expect(a).not.toBe(b);
    } finally {
      restore();
    }
  });

  it("survives a completely absent crypto global", () => {
    const restore = withCrypto(undefined);
    try {
      expect(randomUUID()).toMatch(UUID_V4);
    } finally {
      restore();
    }
  });
});

// ── Regression guard ──────────────────────────────────────────────────
// A bare `crypto.randomUUID()` throws in the remote web client (insecure
// context), taking the whole click handler down with it. Every mint must
// go through `@/lib/uuid` — or carry its own inline guard.
describe("no bare crypto.randomUUID call sites in src/", () => {
  const SRC = resolve(__dirname, "..");
  // Qualified forms throw in exactly the same way, so `window.`,
  // `globalThis.` and `self.` are guarded too — and with no negative
  // lookbehind any other receiver is caught as well, which is the safe
  // direction for a regression guard.
  const BARE = /\b(?:(?:window|globalThis|self)\.)?crypto\.randomUUID\(/;

  function collect(dir: string, out: string[]): string[] {
    for (const entry of readdirSync(dir)) {
      if (entry === "node_modules") continue;
      const full = join(dir, entry);
      if (statSync(full).isDirectory()) {
        collect(full, out);
        continue;
      }
      if (!/\.tsx?$/.test(entry)) continue;
      if (/\.(test|bench)\.tsx?$/.test(entry)) continue;
      if (full === join(SRC, "lib", "uuid.ts")) continue;
      out.push(full);
    }
    return out;
  }

  it("finds none", () => {
    const offenders: string[] = [];
    for (const file of collect(SRC, [])) {
      const lines = readFileSync(file, "utf8").split("\n");
      lines.forEach((line, i) => {
        if (!BARE.test(line)) return;
        // An inline guard on the same line is fine.
        if (line.includes('"randomUUID" in crypto')) return;
        if (line.includes("typeof crypto")) return;
        offenders.push(`${file}:${i + 1}: ${line.trim()}`);
      });
    }
    expect(offenders).toEqual([]);
  });
});
