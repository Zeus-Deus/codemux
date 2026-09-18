import { useMobileNavigationStore } from "@/stores/mobile-navigation-store";
import { useEffect, useRef } from "react";
import { useAppStore } from "@/stores/app-store";
import { activatePane, activateWorkspace } from "@/tauri/commands";
import { remoteViewHost } from "@/remote/client-view";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { toast } from "@/lib/toast";
export function useNotificationLink() {
  const ready = useAppStore((s) => s.appState !== null);
  const handled = useRef(false);
  useEffect(() => {
    if (!ready || handled.current || !isRemoteClient()) return;
    const params = new URLSearchParams(location.search);
    const workspace = params.get("workspace");
    if (!workspace) return;
    handled.current = true;
    if (params.get("device") && params.get("device") !== remoteViewHost()) {
      toast.info(
        "This notification belongs to another desktop. Open its device to continue.",
      );
      return;
    }
    if (
      !useAppStore
        .getState()
        .appState?.workspaces.some((w) => w.workspace_id === workspace)
    ) {
      toast.info("This workspace is no longer open on the desktop.");
      return;
    }
    void activateWorkspace(workspace)
      .then(async () => {
        const pane = params.get("pane");
        if (pane) await activatePane(pane);
        useMobileNavigationStore.getState().setHome(false);
        const url = new URL(location.href);
        url.searchParams.delete("workspace");
        url.searchParams.delete("pane");
        history.replaceState(history.state, "", url);
      })
      .catch((error) =>
        toast.error("Could not open notification", {
          description: String(error),
        }),
      );
  }, [ready]);
}
