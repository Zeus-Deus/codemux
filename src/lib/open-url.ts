/**
 * Opening a link, with one exception.
 *
 * Everything goes to the system browser, except a pull-request URL for a
 * repository the user already has open — which goes to the Pull Requests
 * page, because that page can show the diff, the checks and the review
 * bar, and the browser can only show the same thing further away.
 *
 * Three ways out of the exception, in order:
 *
 * 1. **Shift-click.** The universal "I meant the other one". Checked
 *    first so it beats every other rule, including the setting.
 * 2. **The setting.** Settings ▸ Source control ▸ "Open pull request
 *    links in the browser" turns the whole thing off.
 * 3. **Nothing to open.** If no open project has that pull request, the
 *    page would show an empty selection, so the browser gets it and a
 *    toast says why rather than leaving the click looking broken
 *    (binding rule 5).
 *
 * Buttons whose label already says "Open in browser" deliberately keep
 * calling `openUrl` directly. Interception is for links, not for a
 * control the user pressed *because* it said browser.
 */

import { openUrl } from "@tauri-apps/plugin-opener";

import { toast } from "@/lib/toast";
import { parsePrUrl, resolvePrLink } from "@/lib/pr-url";
import { resolveProvider } from "@/lib/source-control";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { useUIStore } from "@/stores/ui-store";

export interface OpenUrlOptions {
  /** The click, when there was one. Shift means "the browser, please". */
  event?: { shiftKey?: boolean } | null;
}

/**
 * Where a URL is going, decided but not yet acted on.
 *
 * Separate from the acting so a caller that wraps link-opening in its
 * own confirmation (the chat transcript does) can skip that confirmation
 * for a destination inside the app — there is nothing to confirm about
 * switching to a page of Codemux.
 */
export type PrLinkRoute =
  | { kind: "in-app"; projectRoot: string; number: number }
  /** A pull-request URL for a repository no open project holds. */
  | { kind: "unknown-repo"; slug: string; noun: string }
  /** Not a pull-request URL, or the user has opted out. */
  | { kind: "browser" };

export function routeForUrl(url: string, options: OpenUrlOptions = {}): PrLinkRoute {
  if (options.event?.shiftKey) return { kind: "browser" };

  const settings = useSyncedSettingsStore.getState().settings.source_control;
  if (settings?.open_pr_links_in_browser) return { kind: "browser" };

  const parsed = parsePrUrl(url, settings?.custom_hosts);
  if (!parsed) return { kind: "browser" };

  const row = resolvePrLink(parsed);
  if (!row) {
    return {
      kind: "unknown-repo",
      slug: parsed.slug,
      noun: resolveProvider(parsed.kind).noun,
    };
  }
  return { kind: "in-app", projectRoot: row.projectRoot, number: row.number };
}

/** Where a call ended up — returned so callers (and tests) can tell. */
export type OpenUrlOutcome = "in-app" | "browser";

function browser(url: string): Promise<OpenUrlOutcome> {
  return openUrl(url)
    .then(() => "browser" as const)
    .catch((err) => {
      toast.error("Couldn't open the link", { description: String(err) });
      return "browser" as const;
    });
}

/**
 * Open a URL the way the user meant it.
 *
 * Always resolves — a link that cannot be opened raises a toast rather
 * than an unhandled rejection in a click handler.
 */
export async function openExternalUrl(
  url: string,
  options: OpenUrlOptions = {},
): Promise<OpenUrlOutcome> {
  const route = routeForUrl(url, options);

  if (route.kind === "in-app") {
    useUIStore
      .getState()
      .setShowPullRequests(true, {
        projectRoot: route.projectRoot,
        number: route.number,
      });
    return "in-app";
  }

  if (route.kind === "unknown-repo") {
    toast.info(`Opening this ${route.noun} in the browser`, {
      description: `${route.slug} isn't a project you have open in Codemux.`,
    });
  }

  return browser(url);
}
