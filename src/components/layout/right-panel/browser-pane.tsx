/**
 * The right-panel deck's `browser` pane.
 *
 * This is not an embedded copy of the browser — it is *the* browser. The
 * workspace has exactly one agent browser session (`AgentBrowserSession`,
 * keyed by workspace and named `cli_session_name`), and every
 * `codemux browser …` CLI call and MCP tool resolves to it. Mounting
 * `BrowserPane` against that name here means the pane the user drives and
 * the Chromium the agent drives are the same daemon on the same port, with
 * the same cookies and the same page.
 *
 * That works because `BrowserPane` is position-independent: it is a
 * `<canvas>` fed by a WebSocket screencast, not a native webview welded to
 * a slot in the pane tree. `BrowserPeekOverlay` already mounts one outside
 * the pane tree; this is the same trick with a permanent home.
 *
 * Docking/undocking is owned by `right-panel.tsx` — see
 * `dock_browser_in_right_panel` in `src-tauri/src/commands/browser.rs`.
 */
import { ArrowLeft, ArrowRight, RotateCw } from "lucide-react";

import { BrowserPane } from "@/components/browser/BrowserPane";
import { runBrowserNav } from "@/components/browser/browser-nav";
import type { AgentBrowserSession } from "@/tauri/types";

import { PaneActionButton } from "./pane-actions";

/**
 * Back / forward / reload for the tab row's pane-action slot.
 *
 * The same three verbs the main-area pane's `BrowserToolbar` shows, routed
 * through the same `runBrowserNav` helper — a click here and a click there
 * are indistinguishable to the daemon. There is no URL field: an address
 * cannot sit honestly in a 36px row it shares with the tabs, so the deck's
 * status foot carries it instead (see `pane-status.ts`).
 */
export function BrowserPaneActions({ sessionName }: { sessionName: string }) {
  return (
    <>
      <PaneActionButton
        label="Back"
        icon={ArrowLeft}
        testId="browser-pane-back"
        onClick={() => runBrowserNav(sessionName, "back").catch(console.error)}
      />
      <PaneActionButton
        label="Forward"
        icon={ArrowRight}
        testId="browser-pane-forward"
        onClick={() =>
          runBrowserNav(sessionName, "forward").catch(console.error)
        }
      />
      <PaneActionButton
        label="Reload"
        icon={RotateCw}
        testId="browser-pane-reload"
        onClick={() => runBrowserNav(sessionName, "reload").catch(console.error)}
      />
    </>
  );
}

/**
 * The pane body. `session` is null for the beat between opening the tab and
 * `dock_browser_in_right_panel` returning — the session name is minted by
 * the backend (it is derived from the workspace cwd so Chromium storage
 * survives restarts), so there is nothing to connect to until it lands.
 */
export function RightPanelBrowserPane({
  session,
  workspaceId,
}: {
  session: AgentBrowserSession | null;
  workspaceId: string;
}) {
  if (!session) {
    return (
      <div
        data-testid="browser-pane-connecting"
        className="flex h-full items-center justify-center px-6 text-center text-[11.5px] text-muted-foreground"
      >
        Starting browser…
      </div>
    );
  }
  return (
    <div className="h-full" data-testid="right-panel-browser-pane">
      <BrowserPane
        browserId={session.cli_session_name}
        workspaceId={workspaceId}
        focused
        visible
        hideToolbar
      />
    </div>
  );
}
