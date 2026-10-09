import { useEffect, useState } from "react";
import { getLocalDeviceName } from "@/tauri/commands";

// The hostname never changes while the app runs, so one IPC call serves
// every consumer.
let cached: string | null = null;
let inFlight: Promise<string | null> | null = null;

function loadLocalDeviceName(): Promise<string | null> {
  inFlight ??= getLocalDeviceName()
    .then((name) => {
      cached = typeof name === "string" && name.trim() ? name.trim() : null;
      return cached;
    })
    .catch(() => null);
  return inFlight;
}

/** This machine's hostname, or null until it loads (or if it can't). */
export function useLocalDeviceName(): string | null {
  const [name, setName] = useState<string | null>(cached);
  useEffect(() => {
    if (cached !== null) return;
    let alive = true;
    void loadLocalDeviceName().then((value) => {
      if (alive) setName(value);
    });
    return () => {
      alive = false;
    };
  }, []);
  return name;
}

export function __resetLocalDeviceNameForTests() {
  cached = null;
  inFlight = null;
}
