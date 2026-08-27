import type { WorkspaceSyncView } from "@/tauri/commands";

/**
 * Git-HEAD divergence across the account's synced workspace rows.
 *
 * Two rows that share the same (project_remote, git_branch) but carry
 * different `git_head_sha` values are diverged: the same branch has
 * different commits on different devices. Rows without a sha are ignored —
 * divergence needs a sha on both sides to mean anything.
 *
 * Shared by the Devices page (per-row "diverged" chip) and the sidebar
 * footer's device indicator (amber "needs attention" dot), so both agree on
 * exactly which rows are diverged. Keyed by `WorkspaceSyncView.id`.
 */
export interface DivergenceInfo {
  /** Distinct HEAD shas seen for this branch across the account. */
  forks: number;
  /** Host server ids (null = a local/unknown device) of the OTHER rows. */
  otherHostServerIds: (string | null)[];
}

export function detectDivergedRows(
  rows: readonly WorkspaceSyncView[],
): Map<number, DivergenceInfo> {
  const byKey = new Map<string, WorkspaceSyncView[]>();
  for (const row of rows) {
    if (!row.project_remote || !row.git_branch || !row.git_head_sha) continue;
    const key = `${row.project_remote}::${row.git_branch}`;
    const list = byKey.get(key) ?? [];
    list.push(row);
    byKey.set(key, list);
  }
  const result = new Map<number, DivergenceInfo>();
  for (const list of byKey.values()) {
    if (list.length < 2) continue;
    const shas = new Set(list.map((r) => r.git_head_sha as string));
    if (shas.size < 2) continue;
    for (const row of list) {
      result.set(row.id, {
        forks: shas.size,
        otherHostServerIds: list
          .filter((r) => r.id !== row.id)
          .map((r) => r.host_server_id),
      });
    }
  }
  return result;
}
