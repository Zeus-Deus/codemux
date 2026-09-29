import type { CheckInfo } from "@/tauri/types";
import { checkState } from "@/components/workspace/review/review-ui";

export const PR_PENDING_POLL_MS = 30_000;
export const PR_SETTLED_POLL_MS = 120_000;
export const PR_CONVERSATION_POLL_MS = 120_000;
export const EMPTY_CHECKS_GRACE_MS = 60_000;

/** Empty checks get one minute to register after opening a newly pushed PR. */
export function checksAreSettled(
  checks: CheckInfo[] | undefined,
  checksFailed: boolean,
  row: { checks?: string | null; state?: string | null },
  watchedForMs: number,
): boolean {
  if (checksFailed) return true;
  if (!checks) return false;
  if (checks.length === 0) {
    if (row.checks === "none") return true;
    if (row.state != null && row.state.toUpperCase() !== "OPEN") return true;
    return watchedForMs >= EMPTY_CHECKS_GRACE_MS;
  }
  return !checks.some((check) => checkState(check.conclusion, check.status) === "running");
}
