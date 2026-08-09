export type TerminalThemeMode = "app" | "system";

let mode: TerminalThemeMode = "app";
const listeners = new Set<() => void>();

export function getTerminalThemeMode(): TerminalThemeMode {
  return mode;
}

export function setTerminalThemeMode(value: string | null | undefined) {
  const next: TerminalThemeMode = value === "system" ? "system" : "app";
  if (next === mode) return;
  mode = next;
  for (const listener of listeners) listener();
}

export function subscribeTerminalThemeMode(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}
