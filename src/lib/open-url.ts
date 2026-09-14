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
 * 1. **A browser gesture.** Ctrl/Cmd-click, Shift-click or a middle
 *    click — the ones every browser already reads as "somewhere else,
 *    please". Checked before the repository lookup, so asking for the
 *    browser never earns a toast explaining why you got it.
 * 2. **The setting.** Settings ▸ Source control ▸ "Open pull request
 *    links in the browser" turns the whole thing off.
 * 3. **Not your repository.** If no open project is that repository, the
 *    page has nothing to show, so the browser gets it and a toast says
 *    why rather than leaving the click looking broken (binding rule 5).
 *
 * The routing is by *repository*, not by whether the last poll happened
 * to see that pull request: the list refreshes every couple of minutes,
 * so a link to a pull request an agent opened thirty seconds ago would
 * otherwise miss. The page looks up a number it doesn't hold yet.
 *
 * Buttons whose label already says "Open in browser" deliberately keep
 * calling `openUrl` directly. Interception is for links, not for a
 * control the user pressed *because* it said browser.
 */

import { openUrl } from "@tauri-apps/plugin-opener";

import { toast } from "@/lib/toast";
import { parsePrUrl, resolvePrRoot } from "@/lib/pr-url";
import { resolveProvider } from "@/lib/source-control";
import { useAppStore } from "@/stores/app-store";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { useUIStore } from "@/stores/ui-store";

/** The parts of a click that say where the user wants a link to go. */
export interface LinkGesture {
  shiftKey?: boolean;
  ctrlKey?: boolean;
  metaKey?: boolean;
  /** `1` is the middle button. */
  button?: number;
}

export interface OpenUrlOptions {
  /** The click, when there was one. */
  event?: LinkGesture | null;
}

/** Ctrl/Cmd-click, Shift-click or middle click: "the browser, please". */
export function wantsBrowser(event: LinkGesture | null | undefined): boolean {
  return !!event && !!(event.ctrlKey || event.metaKey || event.shiftKey || event.button === 1);
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
  /** Not a pull-request URL, a browser gesture, or the user opted out. */
  | { kind: "browser" };

export function routeForUrl(url: string, options: OpenUrlOptions = {}): PrLinkRoute {
  if (wantsBrowser(options.event)) return { kind: "browser" };

  const settings = useSyncedSettingsStore.getState().settings.source_control;
  if (settings?.open_pr_links_in_browser) return { kind: "browser" };

  const parsed = parsePrUrl(url, settings?.custom_hosts);
  if (!parsed) return { kind: "browser" };

  const known = (useAppStore.getState().appState?.workspaces ?? []).flatMap((ws) => {
    const projectRoot = ws.project_root ?? ws.cwd;
    return ws.pr_url && projectRoot ? [{ url: ws.pr_url, projectRoot }] : [];
  });
  const projectRoot = resolvePrRoot(parsed, undefined, known);
  if (!projectRoot) {
    return {
      kind: "unknown-repo",
      slug: parsed.slug,
      noun: resolveProvider(parsed.kind).noun,
    };
  }
  return { kind: "in-app", projectRoot, number: parsed.number };
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
        url,
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
