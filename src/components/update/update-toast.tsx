import { useEffect, useRef, useState } from "react";
import { toast as sonnerToast } from "sonner";
import { ArrowUpCircle, Check, Copy, TriangleAlert, Wifi } from "lucide-react";
import { useUpdateChecker } from "@/hooks/use-update-checker";
import { Button } from "@/components/ui/button";
import { openUrl } from "@tauri-apps/plugin-opener";
import { COPY_FAILED_MESSAGE, copyToClipboard } from "@/lib/clipboard";
import { manualUpdateCommand, releasePageUrl } from "@/lib/update-release";

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
    <p className="mb-2.5 flex items-start gap-1.5 text-label text-muted-foreground">
      <Wifi className="mt-0.5 size-3 shrink-0" />
      <span>Remote devices are connected — restarting will briefly disconnect them.</span>
    </p>
  );
}

/**
 * The command a package-manager install runs to update, with a copy button.
 * Holds its own "copied" state so the confirmation survives toast re-renders.
 */
function UpdateCommand({ command }: { command: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    if (await copyToClipboard(command)) {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } else {
      sonnerToast.error(COPY_FAILED_MESSAGE);
    }
  };
  return (
    <div className="mb-3 flex items-center gap-1 rounded-md bg-surface-1 py-0.5 pr-0.5 pl-2.5">
      <code className="flex-1 truncate font-mono text-label select-all">
        {command}
      </code>
      <Button
        size="icon-xs"
        variant="ghost"
        aria-label={copied ? "Copied" : "Copy command"}
        title={copied ? "Copied" : "Copy command"}
        onClick={copy}
        className="text-muted-foreground hover:text-foreground"
      >
        {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
      </Button>
    </div>
  );
}

export function UpdateToast() {
  const {
    state,
    updateVersion,
    downloadProgress,
    canAutoUpdate,
    packageFormat,
    errorMessage,
    startDownload,
    installAndRestart,
    retry,
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
    (state === "update-available" ||
      state === "downloading" ||
      state === "ready" ||
      state === "error");

  useEffect(() => {
    if (!visible) {
      if (toastId.current !== undefined) {
        sonnerToast.dismiss(toastId.current);
        toastId.current = undefined;
      }
      return;
    }

    const openRelease = () => openUrl(releasePageUrl(updateVersion));

    const render = () => {
      // Remote (browser) client: no updater plugin. Offer to ask the desktop
      // to update + restart itself; the desktop runs its own standard flow.
      if (isRemote) {
        return (
          <ToastShell>
            <div className="flex items-center gap-2 mb-1">
              <ArrowUpCircle className="size-4 text-primary shrink-0" />
              <p className="text-body font-semibold">Desktop update available</p>
            </div>
            <p className="text-label text-muted-foreground mb-3.5">
              Codemux v{updateVersion} is ready on the desktop
            </p>
            {updateRequested ? (
              <p className="text-label text-muted-foreground">
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
            <p className="text-body font-semibold mb-2.5">Downloading update…</p>
            <div className="bg-muted rounded-full h-2 overflow-hidden">
              <div
                className="bg-primary h-full rounded-full transition-[width] duration-250"
                style={{ width: `${downloadProgress}%` }}
              />
            </div>
            <p className="text-label text-muted-foreground mt-2 tabular-nums">
              {downloadProgress}%
            </p>
          </ToastShell>
        );
      }

      if (state === "ready") {
        return (
          <ToastShell>
            <p className="text-body font-semibold mb-1">Update ready</p>
            <p className="text-label text-muted-foreground mb-3">
              Restart to apply v{updateVersion}
            </p>
            {remoteClientsConnected && <RemoteConnectedHint />}
            <div className="flex gap-2">
              <Button
                size="sm"
                className="flex-1 bg-foreground text-background hover:bg-foreground/90"
                onClick={installAndRestart}
              >
                Restart Now
              </Button>
              {/* The app-menu footer keeps "Restart to update" reachable. */}
              <Button size="sm" variant="ghost" onClick={dismiss}>
                Later
              </Button>
            </div>
          </ToastShell>
        );
      }

      if (state === "error") {
        return (
          <ToastShell>
            <div className="flex items-center gap-2 mb-1">
              <TriangleAlert className="size-4 text-status-attention shrink-0" />
              <p className="text-body font-semibold">Update failed</p>
            </div>
            <p className="text-label text-muted-foreground mb-2">
              Codemux v{updateVersion} couldn't be installed.
            </p>
            {errorMessage && (
              <p
                className="mb-3 line-clamp-3 rounded-md bg-surface-1 px-2.5 py-1.5 font-mono text-caption break-words text-muted-foreground"
                title={errorMessage}
              >
                {errorMessage}
              </p>
            )}
            <div className="flex gap-2">
              <Button
                size="sm"
                className="flex-1 bg-foreground text-background hover:bg-foreground/90"
                onClick={retry}
              >
                Retry
              </Button>
              <Button size="sm" variant="ghost" onClick={openRelease}>
                Download manually
              </Button>
              <Button size="sm" variant="ghost" onClick={dismiss}>
                Dismiss
              </Button>
            </div>
          </ToastShell>
        );
      }

      // update-available. The in-app updater can only replace AppImage and
      // NSIS installs; a package manager owns every other install, so those
      // get the command (when known) or the release page instead.
      const command = canAutoUpdate ? null : manualUpdateCommand(packageFormat);
      return (
        <ToastShell>
          <div className="flex items-center gap-2 mb-1">
            <ArrowUpCircle className="size-4 text-primary shrink-0" />
            <p className="text-body font-semibold">Update available</p>
          </div>
          <p className="text-label text-muted-foreground mb-3">
            {canAutoUpdate
              ? `Codemux v${updateVersion} is ready to install`
              : command
                ? `Codemux v${updateVersion} is out. Codemux was installed with pacman, so update it from a terminal:`
                : `Codemux v${updateVersion} is out. Update it with the package manager you installed it with, or download it from the release page.`}
          </p>
          {command && <UpdateCommand command={command} />}
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
                onClick={openRelease}
              >
                {command ? "What's new" : "Download"}
              </Button>
            )}
            {canAutoUpdate && (
              <Button size="sm" variant="ghost" onClick={openRelease}>
                What's new
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
    packageFormat,
    errorMessage,
    startDownload,
    installAndRestart,
    retry,
    dismiss,
    isRemote,
    remoteClientsConnected,
    requestDesktopUpdate,
    updateRequested,
  ]);

  return null;
}
