import { useEffect, useMemo } from "react";
import { create } from "zustand";

import { useAppStore } from "@/stores/app-store";
import { useSidebarInboxStore } from "@/stores/sidebar-inbox-store";
import { workspacesWorktreeSizes } from "@/tauri/commands";

/**
 * Which settled workspaces the "This device" sweep may remove, and how much
 * disk they hold.
 *
 * Eligibility is the backend's call — `workspaces_worktree_sizes` answers
 * only for disposable local worktrees — so the frontend never re-derives
 * "is this a repo root / attach-only / host-backed" rules that the removal
 * command enforces anyway. Sizes are cached per workspace id: a settled
 * worktree's contents don't change, so one disk walk per id is enough
 * until it leaves the settled shelf (active again, closed, or swept).
 */

/** A settled workspace the sweep may remove. */
export interface SweepCandidate {
  id: string;
  title: string;
  /** Worktree bytes; null when the backend couldn't measure it. */
  bytes: number | null;
}

/** Backend answer per asked id: bytes, null (qualifies, size unknown), or
 *  false (does not qualify). An id absent here hasn't been asked yet. */
type SizeEntry = number | null | false;

interface WorktreeSizesStore {
  entries: Record<string, SizeEntry>;
}

const useWorktreeSizesStore = create<WorktreeSizesStore>(() => ({ entries: {} }));

const pending = new Set<string>();

async function requestSizes(ids: readonly string[]): Promise<void> {
  const { entries } = useWorktreeSizesStore.getState();
  const missing = ids.filter((id) => !(id in entries) && !pending.has(id));
  if (missing.length === 0) return;
  for (const id of missing) pending.add(id);
  try {
    const sizes = await workspacesWorktreeSizes(missing);
    useWorktreeSizesStore.setState((s) => {
      const next = { ...s.entries };
      for (const id of missing) next[id] = id in sizes ? sizes[id] : false;
      return { entries: next };
    });
  } catch {
    // Left unknown; the next settled-set change asks again.
  } finally {
    for (const id of missing) pending.delete(id);
  }
}

/** Forget cached answers — after a sweep, or when a worktree leaves the
 *  settled shelf and may change on disk again. */
export function evictWorktreeSizes(ids: readonly string[]): void {
  if (ids.length === 0) return;
  useWorktreeSizesStore.setState((s) => {
    const next = { ...s.entries };
    for (const id of ids) delete next[id];
    return { entries: next };
  });
}

export function __resetWorktreeSizesForTests(): void {
  pending.clear();
  useWorktreeSizesStore.setState({ entries: {} });
}

export interface SweepCandidates {
  candidates: SweepCandidate[];
  /** Sum of the measured candidates; null until at least one is known. */
  knownBytes: number | null;
}

/** Ids and titles as one string so an unrelated app-state emit doesn't
 *  rebuild the candidate list. */
function selectWorkspaceKey(state: {
  appState: { workspaces: readonly { workspace_id: string; title: string }[] } | null;
}): string {
  return (state.appState?.workspaces ?? [])
    .map((ws) => `${ws.workspace_id}\t${ws.title}`)
    .join("\n");
}

export function useSweepCandidates(): SweepCandidates {
  const settled = useSidebarInboxStore((s) => s.settled);
  const loadInbox = useSidebarInboxStore((s) => s.load);
  const workspaceKey = useAppStore(selectWorkspaceKey);
  const entries = useWorktreeSizesStore((s) => s.entries);

  // The settled list normally loads with the sidebar; the Devices page
  // replaces the shell, so make sure it is loaded here too (idempotent).
  useEffect(() => {
    void loadInbox();
  }, [loadInbox]);

  // Settled ids that still exist locally, in shelf order, with titles.
  const settledIds = useMemo(() => {
    const titleById = new Map<string, string>();
    if (workspaceKey) {
      for (const line of workspaceKey.split("\n")) {
        const [id, title] = line.split("\t");
        titleById.set(id, title);
      }
    }
    return settled.flatMap((entry) => {
      const title = titleById.get(entry.id);
      return title === undefined ? [] : [{ id: entry.id, title }];
    });
  }, [settled, workspaceKey]);

  const settledKey = settledIds.map((s) => s.id).join("\n");
  useEffect(() => {
    const ids = settledKey ? settledKey.split("\n") : [];
    const keep = new Set(ids);
    evictWorktreeSizes(
      Object.keys(useWorktreeSizesStore.getState().entries).filter(
        (id) => !keep.has(id),
      ),
    );
    void requestSizes(ids);
  }, [settledKey]);

  return useMemo(() => {
    const candidates: SweepCandidate[] = [];
    let knownBytes: number | null = null;
    for (const { id, title } of settledIds) {
      const entry = entries[id];
      if (entry === undefined || entry === false) continue;
      candidates.push({ id, title, bytes: entry });
      if (entry !== null) knownBytes = (knownBytes ?? 0) + entry;
    }
    return { candidates, knownBytes };
  }, [settledIds, entries]);
}
