import { useEffect, useRef } from "react";
import { toast as sonnerToast } from "sonner";
import { ArrowUpCircle, Wifi } from "lucide-react";
import { useUpdateChecker } from "@/hooks/use-update-checker";
import { Button } from "@/components/ui/button";
import { openUrl } from "@tauri-apps/plugin-opener";

/**
 * Opaque card shell for the update toast.
 *
 * Custom sonner toasts (`toast.custom`) do NOT inherit the `--normal-bg`
 * styling the `<Toaster>` applies to standard toasts, so without an
 * explicit background the toast renders see-through and unreadable. This
 * shell pins a solid `bg-popover` surface with a border and shadow.
 */
function ToastShell({ children }: { children: React.ReactNode }) {
  return (
    <div className="w-[340px] rounded-lg border border-border bg-popover px-4 py-3.5 text-popover-foreground shadow-lg">
      {children}
    </div>
  );
}

/**
 * Hint shown when a paired remote device is attached: restarting the desktop
 * to apply an update briefly disconnects those devices, so the restart is a
 * deliberate choice rather than an automatic one (the update-while-remote
 * defer policy). Desktop-only.
 */
function RemoteConnectedHint() {
  return (
    <p className="mb-2.5 flex items-start gap-1.5 text-xs text-muted-foreground">
      <Wifi className="mt-0.5 size-3 shrink-0" />
      <span>Remote devices are connected — restarting will briefly disconnect them.</span>
    </p>
  );
}

export function UpdateToast() {
  const {
    state,
    updateVersion,
    downloadProgress,
    canAutoUpdate,
    startDownload,
    installAndRestart,
    dismiss,
    dismissed,
    isRemote,
    remoteClientsConnected,
    requestDesktopUpdate,
    updateRequested,
  } = useUpdateChecker();

  const toastId = useRef<string | number | undefined>(undefined);

  const visible =
    !dismissed &&
    (state === "update-available" || state === "downloading" || state === "ready");

  useEffect(() => {
    if (!visible) {
      if (toastId.current !== undefined) {
        sonnerToast.dismiss(toastId.current);
        toastId.current = undefined;
      }
      return;
    }

    const render = () => {
      // Remote (browser) client: no updater plugin. Offer to ask the desktop
      // to update + restart itself; the desktop runs its own standard flow.
      if (isRemote) {
        return (
          <ToastShell>
            <div className="flex items-center gap-2 mb-1">
              <ArrowUpCircle className="size-4 text-primary shrink-0" />
              <p className="text-sm font-semibold">Desktop update available</p>
            </div>
            <p className="text-xs text-muted-foreground mb-3.5">
              Codemux v{updateVersion} is ready on the desktop
            </p>
            {updateRequested ? (
              <p className="text-xs text-muted-foreground">
                Update requested — the desktop is restarting. This device will
                reconnect automatically.
              </p>
            ) : (
              <div className="flex gap-2">
                <Button
                  size="sm"
                  className="flex-1 bg-foreground text-background hover:bg-foreground/90"
                  onClick={requestDesktopUpdate}
                >
                  Update &amp; restart desktop
                </Button>
                <Button size="sm" variant="ghost" onClick={dismiss}>
                  Later
                </Button>
              </div>
            )}
          </ToastShell>
        );
      }

      if (state === "downloading") {
        return (
          <ToastShell>
            <p className="text-sm font-semibold mb-2.5">Downloading update…</p>
            <div className="bg-muted rounded-full h-2 overflow-hidden">
              <div
                className="bg-primary h-full rounded-full transition-all duration-200"
                style={{ width: `${downloadProgress}%` }}
              />
            </div>
            <p className="text-xs text-muted-foreground mt-2">{downloadProgress}%</p>
          </ToastShell>
        );
      }

      if (state === "ready") {
        return (
          <ToastShell>
            <p className="text-sm font-semibold mb-1">Update ready</p>
            <p className="text-xs text-muted-foreground mb-3">
              Restart to apply v{updateVersion}
            </p>
            {remoteClientsConnected && <RemoteConnectedHint />}
            <Button
              size="sm"
              className="w-full bg-foreground text-background hover:bg-foreground/90"
              onClick={installAndRestart}
            >
              Restart Now
            </Button>
          </ToastShell>
        );
      }

      // update-available
      return (
        <ToastShell>
          <div className="flex items-center gap-2 mb-1">
            <ArrowUpCircle className="size-4 text-primary shrink-0" />
            <p className="text-sm font-semibold">Update available</p>
          </div>
          <p className="text-xs text-muted-foreground mb-3.5">
            Codemux v{updateVersion} is ready to install
          </p>
          {canAutoUpdate && remoteClientsConnected && <RemoteConnectedHint />}
          <div className="flex gap-2">
            {canAutoUpdate ? (
              <Button
                size="sm"
                className="flex-1 bg-foreground text-background hover:bg-foreground/90"
                onClick={startDownload}
              >
                Install &amp; Restart
              </Button>
            ) : (
              <Button
                size="sm"
                className="flex-1 bg-foreground text-background hover:bg-foreground/90"
                onClick={() =>
                  openUrl(
                    `https://github.com/Zeus-Deus/codemux/releases/tag/v${updateVersion}`,
                  )
                }
              >
                Download
              </Button>
            )}
            <Button size="sm" variant="ghost" onClick={dismiss}>
              Later
            </Button>
          </div>
        </ToastShell>
      );
    };

    toastId.current = sonnerToast.custom(render, {
      id: toastId.current ?? "codemux-update",
      duration: Infinity,
    });
  }, [
    visible,
    state,
    updateVersion,
    downloadProgress,
    canAutoUpdate,
    startDownload,
    installAndRestart,
    dismiss,
    isRemote,
    remoteClientsConnected,
    requestDesktopUpdate,
    updateRequested,
  ]);

  return null;
}
