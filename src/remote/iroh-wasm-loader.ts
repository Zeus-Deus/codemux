/**
 * Lazy loader for the optional iroh WASM client.
 *
 * The relay ("from anywhere") transport needs an in-browser iroh endpoint, which
 * ships as a WebAssembly module built by `scripts/build-iroh-wasm.sh` into
 * `public/iroh-wasm/`. That artifact is multi-MB and **not committed** — the main
 * build stays green without it, and the LAN/mesh WebSocket path is the default.
 *
 * This module imports the artifact at runtime, on first relay connect, behind a
 * runtime-computed specifier so neither `tsc` nor `vite build` tries to resolve
 * it at build time. When the artifact is absent the import rejects and we throw a
 * typed {@link IrohWasmUnavailableError} so the bootstrap can show
 * "relay client unavailable — build the iroh wasm" instead of a raw error.
 */
import type { IrohByteStream, IrohDialer } from "./iroh-transport";

/** Thrown when the iroh WASM artifact is missing or fails to initialize — the
 *  bootstrap treats this as "build the wasm to use relay mode", not a crash. */
export class IrohWasmUnavailableError extends Error {
  constructor(readonly cause?: unknown) {
    super(
      "iroh relay client unavailable — build it with scripts/build-iroh-wasm.sh",
    );
    this.name = "IrohWasmUnavailableError";
  }
}

/** Served paths of the built artifact (from `public/iroh-wasm/`). */
const ARTIFACT_JS = "/iroh-wasm/iroh_wasm.js";
const ARTIFACT_WASM = "/iroh-wasm/iroh_wasm_bg.wasm";

/** One open stream as exposed by the wasm-bindgen glue. `read` resolves to the
 *  next chunk, or `null`/`undefined` at EOF. */
interface WasmStream {
  write(data: Uint8Array): Promise<void>;
  read(): Promise<Uint8Array | null | undefined>;
  close(): void;
}

/** The shape the wasm-bindgen glue module exposes (see `iroh-wasm/src/lib.rs`).
 *  `default` is the wasm-bindgen init (`__wbg_init`, which takes an options
 *  object `{module_or_path}` in the pinned 0.2.126 glue); `connect` dials a node
 *  and opens a bi-stream. */
interface WasmModule {
  default(options?: { module_or_path: string }): Promise<unknown>;
  connect(nodeId: string, relayUrl?: string): Promise<WasmStream>;
}

/** Import a module by a runtime specifier. Injectable so tests can simulate a
 *  present or absent artifact without a real dynamic import. */
export type WasmImporter = (specifier: string) => Promise<unknown>;

/** Default importer: a genuinely dynamic import the bundler leaves external. */
const defaultImporter: WasmImporter = (specifier) =>
  import(/* @vite-ignore */ specifier);

/** Memoized so the wasm is fetched + initialized at most once per page. */
let dialerPromise: Promise<IrohDialer> | null = null;

/**
 * Load (once) and return the iroh {@link IrohDialer}. Rejects with
 * {@link IrohWasmUnavailableError} when the artifact can't be loaded. A failed
 * attempt clears the memo so a later retry (e.g. after the artifact is deployed)
 * can succeed.
 */
export function loadIrohDialer(
  importer: WasmImporter = defaultImporter,
): Promise<IrohDialer> {
  if (!dialerPromise) {
    dialerPromise = buildDialer(importer).catch((err) => {
      dialerPromise = null;
      throw err instanceof IrohWasmUnavailableError
        ? err
        : new IrohWasmUnavailableError(err);
    });
  }
  return dialerPromise;
}

/** Reset the memo (tests). */
export function __resetIrohDialer(): void {
  dialerPromise = null;
}

async function buildDialer(importer: WasmImporter): Promise<IrohDialer> {
  let mod: WasmModule;
  try {
    mod = (await importer(ARTIFACT_JS)) as WasmModule;
  } catch (err) {
    throw new IrohWasmUnavailableError(err);
  }
  if (typeof mod?.default !== "function" || typeof mod?.connect !== "function") {
    throw new IrohWasmUnavailableError(
      new Error("iroh wasm module missing expected exports"),
    );
  }
  await mod.default({ module_or_path: ARTIFACT_WASM });
  return {
    async dial(target): Promise<IrohByteStream> {
      const handle = await mod.connect(
        target.nodeId,
        target.relayUrl ?? undefined,
      );
      return {
        write: (data) => handle.write(data),
        read: async () => (await handle.read()) ?? null,
        close: () => handle.close(),
      };
    },
  };
}
