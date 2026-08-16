/**
 * The app-level pull-request watcher: two toasts, and the index that
 * lets a pasted pull-request link find its way in.
 *
 * Mounted once, above every screen, from `app-shell` — before the
 * full-screen destinations return, so the watcher keeps running while
 * you are in Settings or on the Pull Requests page itself. It shares its
 * query key with the sidebar badge and the page, so watching costs no
 * extra fetches.
 *
 * Which transitions earn a toast, and why they can't repeat, is in
 * `pr-events.ts`. This file is only the raising of them.
 */

import { useEffect, useRef } from "react";

import { getPullRequestChecks } from "@/tauri/commands";
import { handOffToAgent } from "@/lib/pr-agent-handoff";
import { detectPrEvents, snapshotRows, type PrSnapshot } from "@/lib/pr-events";
import { usePrOverview } from "@/lib/pr-overview-query";
import { publishPrLinkIndex } from "@/lib/pr-url";
import type { PrRow } from "@/lib/pr-overview";
import { resolveProvider } from "@/lib/source-control";
import { checkState } from "@/components/workspace/review/review-ui";
import { toast } from "@/lib/toast";
import { useUIStore } from "@/stores/ui-store";

/** `acme/web#285`, or `#285` when the URL never named the repository. */
function prRefOf(row: PrRow): string {
  const sigil = resolveProvider(row.providerKind).sigil;
  return `${row.repo ? row.repo : ""}${sigil}${row.number}`;
}

function openPage(row: PrRow): void {
  useUIStore
    .getState()
    .setShowPullRequests(true, { projectRoot: row.projectRoot, number: row.number });
}

/**
 * "Review requested · #285 Fix the installer".
 *
 * Info, not error: someone asking for your attention is not a problem,
 * and [Open] is the whole point of raising it.
 */
function reviewRequestedToast(row: PrRow): void {
  toast.info(`Review requested · ${prRefOf(row)}`, {
    description: row.title,
    action: { label: "Open", onClick: () => openPage(row) },
  });
}

/**
 * "CI failed on #285 / rust (ubuntu-latest)" with [Fix].
 *
 * The failing check is named by asking the host once, at the transition
 * — both because the second line is most of the toast's value, and
 * because [Fix] hands an agent a specific check rather than "something
 * is red". When the name can't be had, the toast still fires and offers
 * [Open]: a control that can't do what it says is never drawn.
 */
async function checksFailedToast(row: PrRow): Promise<void> {
  const failing = await getPullRequestChecks(row.projectRoot, row.number)
    .then((checks) => checks.find((c) => checkState(c.conclusion, c.status) === "fail") ?? null)
    .catch(() => null);

  const provider = resolveProvider(row.providerKind);
  const message = `CI failed on ${prRefOf(row)}`;

  if (!failing) {
    toast.error(message, {
      description: row.title,
      action: { label: "Open", onClick: () => openPage(row) },
    });
    return;
  }

  toast.error(message, {
    description: failing.name,
    action: {
      label: "Fix",
      onClick: () => {
        void handOffToAgent({
          pr: {
            number: row.number,
            title: row.title,
            url: row.url,
            head_branch: row.head_branch,
            // The overview row carries no base branch, and naming the
            // wrong one in the prompt is worse than not naming one.
            base_branch: null,
          },
          task: { kind: "failing-check", checkName: failing.name },
          prRef: prRefOf(row),
          projectRoot: row.projectRoot,
          cwd: row.projectRoot,
          cli: provider.cli,
          providerKind: provider.kind,
        }).catch((err) => toast.error("Couldn't start the agent", { description: String(err) }));
      },
    },
  });
}

/**
 * Watch the overview for the two events, and keep the link index fresh.
 *
 * Call once, from the app shell.
 */
export function usePrEventToasts(): void {
  const { rows, viewerByRoot } = usePrOverview(true);
  const previous = useRef<PrSnapshot | null>(null);
  const fired = useRef<ReadonlySet<string>>(new Set<string>());

  useEffect(() => {
    // Pull-request links resolve against whatever the last poll saw.
    publishPrLinkIndex(rows);

    const next = snapshotRows(rows, viewerByRoot);
    const result = detectPrEvents(previous.current, next, fired.current);
    previous.current = next;
    fired.current = result.fired;

    for (const event of result.events) {
      if (event.kind === "review-requested") reviewRequestedToast(event.row);
      else void checksFailedToast(event.row);
    }
  }, [rows, viewerByRoot]);
}
