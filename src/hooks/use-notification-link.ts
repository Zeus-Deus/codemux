import { useEffect, useRef } from "react";
import { useAppStore } from "@/stores/app-store";
import { remoteViewHost } from "@/remote/client-view";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { openNotificationTarget } from "@/lib/open-notification-target";
import { toast } from "@/lib/toast";

/** Remote clients: open the agent a web-push deep link points at
 *  (`?workspace=…&pane=…&device=…`), then drop the params from the URL. */
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
    void openNotificationTarget({
      workspace_id: workspace,
      pane_id: params.get("pane") ?? "",
    }).then((opened) => {
      if (!opened) return;
      const url = new URL(location.href);
      url.searchParams.delete("workspace");
      url.searchParams.delete("pane");
      history.replaceState(history.state, "", url);
    });
  }, [ready]);
}
