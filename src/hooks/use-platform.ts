import { useEffect, useState } from "react";
import { getPlatform } from "@/tauri/commands";

// Module-level cache — the OS never changes during a session, so the first
// successful resolution is shared across every consumer of usePlatform().
// Subsequent hook mounts skip the invoke round-trip entirely.
let cachedPlatform: string | null = null;
let inFlight: Promise<string> | null = null;

function resolvePlatform(): Promise<string> {
  if (cachedPlatform !== null) return Promise.resolve(cachedPlatform);
  if (inFlight) return inFlight;
  inFlight = getPlatform()
    .then((os) => {
      cachedPlatform = os;
      return os;
    })
    .catch((error) => {
      // Default to an empty string so we don't incorrectly label the platform
      // as Windows on failure — false positives would hide OpenFlow for Linux
      // users too.
      console.error("[usePlatform] get_platform invoke failed:", error);
      cachedPlatform = "";
      return "";
    })
    .finally(() => {
      inFlight = null;
    });
  return inFlight;
}

export interface PlatformInfo {
  /** Rust `std::env::consts::OS` string: "linux", "macos", "windows", etc. */
  os: string;
  /** True while the first invoke is still in flight. */
  loading: boolean;
  /** Convenience flag — `os === "windows"`. */
  isWindows: boolean;
}

/**
 * React hook exposing the current OS as reported by the Rust side.
 *
 * Uses a module-level cache so the invoke only happens once per session. The
 * initial render returns `{ os: "", loading: true }`; after the first resolve
 * every subsequent mount returns the cached value synchronously on the first
 * render.
 */
export function usePlatform(): PlatformInfo {
  const [os, setOs] = useState<string>(cachedPlatform ?? "");
  const [loading, setLoading] = useState<boolean>(cachedPlatform === null);

  useEffect(() => {
    if (cachedPlatform !== null) {
      setOs(cachedPlatform);
      setLoading(false);
      return;
    }
    let cancelled = false;
    resolvePlatform().then((resolved) => {
      if (cancelled) return;
      setOs(resolved);
      setLoading(false);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  return { os, loading, isWindows: os === "windows" };
}
