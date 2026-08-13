import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  loadIrohDialer,
  IrohWasmUnavailableError,
  __resetIrohDialer,
  resolveIrohWasmArtifactPaths,
  type WasmImporter,
} from "./iroh-wasm-loader";

beforeEach(() => __resetIrohDialer());

describe("resolveIrohWasmArtifactPaths", () => {
  it("keeps the unversioned path for local builds", () => {
    expect(resolveIrohWasmArtifactPaths()).toEqual({
      js: "/iroh-wasm/iroh_wasm.js",
      wasm: "/iroh-wasm/iroh_wasm_bg.wasm",
    });
  });

  it("isolates hosted artifacts by release tag", () => {
    expect(resolveIrohWasmArtifactPaths("v0.20.0")).toEqual({
      js: "/iroh-wasm/v0.20.0/iroh_wasm.js",
      wasm: "/iroh-wasm/v0.20.0/iroh_wasm_bg.wasm",
    });
  });

  it("rejects an unsafe release path", () => {
    expect(() => resolveIrohWasmArtifactPaths("../latest")).toThrow(
      "invalid Codemux release tag",
    );
  });
});

describe("loadIrohDialer graceful missing-wasm", () => {
  it("throws IrohWasmUnavailableError when the artifact import rejects", async () => {
    const importer: WasmImporter = vi.fn(async () => {
      throw new Error("404 Not Found");
    });
    await expect(loadIrohDialer(importer)).rejects.toBeInstanceOf(
      IrohWasmUnavailableError,
    );
  });

  it("throws IrohWasmUnavailableError when the module lacks expected exports", async () => {
    const importer: WasmImporter = vi.fn(async () => ({ nope: true }));
    await expect(loadIrohDialer(importer)).rejects.toBeInstanceOf(
      IrohWasmUnavailableError,
    );
  });

  it("clears the memo on failure so a later retry can succeed", async () => {
    const bad: WasmImporter = vi.fn(async () => {
      throw new Error("missing");
    });
    await expect(loadIrohDialer(bad)).rejects.toBeInstanceOf(
      IrohWasmUnavailableError,
    );

    const init = vi.fn(async () => undefined);
    const connect = vi.fn(async () => ({
      write: async () => {},
      read: async () => null,
      close: () => {},
    }));
    const good: WasmImporter = vi.fn(async () => ({ default: init, connect }));
    const dialer = await loadIrohDialer(good);
    expect(dialer).toBeTruthy();
    // Init ran with the wasm URL (0.2.126 options-object form).
    expect(init).toHaveBeenCalledWith({
      module_or_path: "/iroh-wasm/iroh_wasm_bg.wasm",
    });
  });

  it("builds a dialer that forwards node_id + relay hint to connect()", async () => {
    const stream = {
      write: vi.fn(async () => {}),
      read: vi.fn(async () => null),
      close: vi.fn(),
    };
    const connect = vi.fn(async () => stream);
    const importer: WasmImporter = vi.fn(async () => ({
      default: async () => undefined,
      connect,
    }));
    const dialer = await loadIrohDialer(importer);
    const bytes = await dialer.dial({ nodeId: "node-9", relayUrl: "https://r" });
    expect(connect).toHaveBeenCalledWith("node-9", "https://r");
    // read() normalizes the wasm's null/undefined EOF to null.
    expect(await bytes.read()).toBeNull();
  });

  it("memoizes so the wasm initializes at most once", async () => {
    const init = vi.fn(async () => undefined);
    const importer: WasmImporter = vi.fn(async () => ({
      default: init,
      connect: async () => ({ write: async () => {}, read: async () => null, close: () => {} }),
    }));
    await Promise.all([loadIrohDialer(importer), loadIrohDialer(importer)]);
    await loadIrohDialer(importer);
    expect(importer).toHaveBeenCalledTimes(1);
    expect(init).toHaveBeenCalledTimes(1);
  });
});
