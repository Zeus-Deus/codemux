/**
 * Live per-session terminal working directories.
 *
 * Deliberately a **frontend-only** store rather than a field on
 * `TerminalSessionSnapshot`. The Rust snapshot's `cwd` is the *spawn*
 * directory: written once at session creation and never refreshed.
 * Turning it into a live value would mean mutating the snapshot on every
 * `cd`, which fans out a full state rebuild + persistence write for a
 * cosmetic header label. Keeping it here costs nothing on the backend and
 * lets the value die with the window, which is correct — it's rederived
 * within one poll interval of the next launch.
 *
 * ## Two sources, one precedence rule
 *
 * - **`osc7`** — the shell emits `ESC ] 7 ; file://host/path ST` on every
 *   prompt. Event-driven, exact, zero polling, and it works for remote/SSH
 *   panes where the pid lives on another machine. Requires shell
 *   integration, which is common but not universal (fish and vte-patched
 *   bash ship it; a bare zsh often doesn't).
 * - **`proc`** — readlink `/proc/<pid>/cwd` on the session's shell pid.
 *   Needs no shell cooperation and is exact on local Linux sessions, but
 *   costs a poll and cannot see a remote host's filesystem.
 *
 * OSC 7 always wins: it is push-based and strictly fresher than a poll
 * that can be up to one interval stale. Once a session has reported via
 * OSC 7, `proc` writes for that session are rejected outright and the
 * poller drops it from its request set — so a shell with integration
 * stops costing IPC entirely.
 */

import { create } from "zustand";

export type CwdSource = "osc7" | "proc";

interface SessionCwd {
  cwd: string;
  source: CwdSource;
}

interface TerminalCwdStore {
  /** sessionId -> last known working directory. */
  cwds: Record<string, SessionCwd>;
  /** Record a cwd for a session, honoring the osc7-wins precedence. */
  setCwd: (sessionId: string, cwd: string, source: CwdSource) => void;
  /** Bulk update from one poll response. Applies the same precedence and
   *  collapses to a single store write so N sessions cause one re-render
   *  pass rather than N. */
  setPolledCwds: (entries: Record<string, string>) => void;
  /** Forget a session (pane/tab/workspace closed). */
  clearCwd: (sessionId: string) => void;
  /** Forget every session NOT in `liveIds`, in a single store write —
   *  closing a workspace with N terminals costs one render pass, not N.
   *  Returns the identical state object when nothing was pruned, so the
   *  steady state stays subscriber-silent (same contract as
   *  `setPolledCwds`). */
  pruneCwds: (liveIds: ReadonlySet<string>) => void;
  /** Session ids that have reported via OSC 7 and therefore never need
   *  polling. Read by the poller to build its request set. */
  osc7SessionIds: () => Set<string>;
}

/** True when an incoming write should replace what's already stored. */
function shouldReplace(existing: SessionCwd | undefined, next: SessionCwd) {
  if (!existing) return true;
  // A poll result must never clobber a push-based OSC 7 value.
  if (existing.source === "osc7" && next.source === "proc") return false;
  return existing.cwd !== next.cwd || existing.source !== next.source;
}

export const useTerminalCwdStore = create<TerminalCwdStore>((set, get) => ({
  cwds: {},

  setCwd: (sessionId, cwd, source) =>
    set((state) => {
      const next = { cwd, source };
      if (!shouldReplace(state.cwds[sessionId], next)) return state;
      return { cwds: { ...state.cwds, [sessionId]: next } };
    }),

  setPolledCwds: (entries) =>
    set((state) => {
      let changed = false;
      const cwds = { ...state.cwds };
      for (const [sessionId, cwd] of Object.entries(entries)) {
        const next: SessionCwd = { cwd, source: "proc" };
        if (!shouldReplace(cwds[sessionId], next)) continue;
        cwds[sessionId] = next;
        changed = true;
      }
      // Returning the same state object keeps zustand from notifying
      // subscribers — the steady state (nobody `cd`s) is free.
      return changed ? { cwds } : state;
    }),

  clearCwd: (sessionId) =>
    set((state) => {
      if (!(sessionId in state.cwds)) return state;
      const cwds = { ...state.cwds };
      delete cwds[sessionId];
      return { cwds };
    }),

  pruneCwds: (liveIds) =>
    set((state) => {
      let changed = false;
      const cwds: Record<string, SessionCwd> = {};
      for (const [sessionId, entry] of Object.entries(state.cwds)) {
        if (liveIds.has(sessionId)) {
          cwds[sessionId] = entry;
        } else {
          changed = true;
        }
      }
      return changed ? { cwds } : state;
    }),

  osc7SessionIds: () => {
    const ids = new Set<string>();
    for (const [sessionId, entry] of Object.entries(get().cwds)) {
      if (entry.source === "osc7") ids.add(sessionId);
    }
    return ids;
  },
}));

/** Subscribe to one session's cwd. Returns a primitive (or null) so the
 *  subscriber re-renders only when that session's directory actually
 *  changes, not on every poll tick. */
export function useTerminalCwd(sessionId: string): string | null {
  return useTerminalCwdStore((s) => s.cwds[sessionId]?.cwd ?? null);
}

/**
 * Parse an OSC 7 payload into a filesystem path.
 *
 * The sequence carries a file URI — `file://<host>/<path>` — where the
 * host is informational (often the hostname, often empty) and the path is
 * percent-encoded. A bare path is also accepted because some shells emit
 * one despite the spec.
 *
 * @returns The decoded absolute path, or null when the payload is not a
 *          usable local path (malformed encoding, or a non-`file:` URI).
 */
export function parseOsc7(payload: string): string | null {
  if (!payload) return null;

  let path: string;
  if (payload.startsWith("file://")) {
    const afterScheme = payload.slice("file://".length);
    // Everything up to the first `/` is the authority (host), which we
    // ignore — an SSH'd shell reports the remote hostname here, and the
    // path is still the one the user cares about. A UNC path serialized
    // with an empty authority (`file:////server/share`) keeps its `//`
    // prefix intact: the first `/` is at index 0, so nothing is dropped
    // and the `//server/share/…` form flows through to the formatter,
    // which normalizes all Windows paths to forward slashes anyway.
    const slash = afterScheme.indexOf("/");
    if (slash === -1) return null;
    path = afterScheme.slice(slash);
  } else if (payload.startsWith("/")) {
    path = payload;
  } else {
    return null;
  }

  let decoded: string;
  try {
    decoded = decodeURIComponent(path);
  } catch {
    // Malformed percent-encoding — better to keep the previous value than
    // to render mojibake in the header.
    return null;
  }

  // A Windows drive path round-trips through a file URI as `/C:/Users/…`:
  // the URI's path component must begin with `/`, so the drive letter
  // picks up a leading slash that was never part of the filesystem path.
  // Strip it, otherwise the formatter can't match the cwd against a
  // `C:\Users\…` workspace root and treats every Windows terminal as
  // living outside its workspace.
  if (/^\/[A-Za-z]:([\\/]|$)/.test(decoded)) return decoded.slice(1);

  return decoded;
}
