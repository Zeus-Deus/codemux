import { randomUUID } from "@/lib/uuid";

let hostKey = typeof location === "undefined" ? "local" : location.origin;

/** Hosted clients share an origin, so scope view state to the selected host. */
export function setRemoteViewHost(host: string): void {
  hostKey = host;
}

function selectionKey(): string {
  return `codemux.remote.workspace:${hostKey}`;
}

export function loadRemoteWorkspace(): string | null {
  try { return localStorage.getItem(selectionKey()); } catch { return null; }
}

export function saveRemoteWorkspace(id: string): void {
  try { localStorage.setItem(selectionKey(), id); } catch { /* storage is optional */ }
}

/** Per page identity survives reconnects; separate tabs remain separate clients. */
let clientId: string | undefined;
export function remoteClientId(): string {
  return clientId ??= randomUUID();
}

export function remoteViewHost(): string { return hostKey; }
