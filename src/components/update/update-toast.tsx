import { useEffect, useRef } from "react";
import { toast as sonnerToast } from "sonner";
import { ArrowUpCircle } from "lucide-react";
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
  ]);

  return null;
}
