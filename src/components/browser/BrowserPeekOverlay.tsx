import { useEffect, useState } from "react";
import { selectBackgroundBrowserSession } from "./background-browser-indicator";
import { BrowserPane, type BrowserStreamStatus } from "./BrowserPane";
import { PeekShell, type PeekContent } from "@/components/peek/PeekShell";
import { PluginPeekOverlay } from "@/components/peek/PluginPeekOverlay";
import { toast } from "@/lib/toast";
import { useGuiChrome } from "@/hooks/use-gui-chrome";
import { useActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { useBrowserPeekStore } from "@/stores/browser-peek-store";
import {
  parseViewportString,
  selectBackgroundBrowserDesktopViewport,
  selectBrowserDefaultViewport,
  useSyncedSettingsStore,
} from "@/stores/synced-settings-store";
import { useUIStore } from "@/stores/ui-store";
import { dockBrowserInRightPanel } from "@/tauri/commands";

// Matches the backend's desktop viewport preset when no default is configured.
const DESKTOP_PEEK_VIEWPORT = { width: 1280, height: 800 };

function BrowserPeek() {
  const guiChrome = useGuiChrome();
  const workspaceId = useActiveWorkspaceId();
  const isOpen = useBrowserPeekStore((s) => workspaceId ? s.isOpen(workspaceId) : false);
  const pluginOpen = useBrowserPeekStore((s) => s.pluginPeek !== null);
  const close = useBrowserPeekStore((s) => s.close);
  const session = useAppStore((s) => selectBackgroundBrowserSession(s.appState, workspaceId));
  const desktopViewport = useSyncedSettingsStore(selectBackgroundBrowserDesktopViewport);
  const defaultViewport = useSyncedSettingsStore(selectBrowserDefaultViewport);
  const pinnedViewport = parseViewportString(defaultViewport) ?? DESKTOP_PEEK_VIEWPORT;
  const [status, setStatus] = useState<BrowserStreamStatus>("starting");
  useEffect(() => {
    const state = useBrowserPeekStore.getState();
    if (state.openWorkspaceId !== null && state.openWorkspaceId !== workspaceId) state.closeAll();
  }, [workspaceId]);
  const content: PeekContent | null = guiChrome && isOpen && session && workspaceId ? {
    id: "browser",
    workspaceId,
    title: session.current_url ?? "about:blank",
    titlePresentation: "url",
    ariaLabel: "Background browser preview",
    status: status === "live" || status === "error" ? status : "connecting",
    statusLabel: status === "error" ? "Disconnected" : undefined,
    close: () => close(workspaceId),
    dock: async () => {
      try {
        await dockBrowserInRightPanel(workspaceId);
        useUIStore.getState().setRightPanelTab(workspaceId, "browser");
        close(workspaceId);
      } catch (error) {
        toast.error("Couldn't open the browser in the side panel", { description: String(error) });
      }
    },
    render: ({ visible }) => <BrowserPane browserId={session.cli_session_name} workspaceId={workspaceId}
      focused={false} visible={visible} hideToolbar
      fixedViewport={desktopViewport ? pinnedViewport : undefined} onStatusChange={setStatus} />,
  } : null;
  return <PeekShell content={content} workspaceId={workspaceId} allowExit={!pluginOpen} />;
}

/** Browser and plugin bodies share chrome and one mutually exclusive open state. */
export function BrowserPeekOverlay() {
  return <><BrowserPeek /><PluginPeekOverlay /></>;
}
