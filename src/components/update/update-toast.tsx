import { useEffect, useRef } from "react";
import { toast as sonnerToast } from "sonner";
import { useUpdateChecker } from "@/hooks/use-update-checker";
import { Button } from "@/components/ui/button";
import { openUrl } from "@tauri-apps/plugin-opener";

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
          <div>
            <p className="text-sm font-medium mb-2">Downloading update...</p>
            <div className="bg-muted rounded-full h-1.5 overflow-hidden">
              <div
                className="bg-primary h-full rounded-full transition-all duration-200"
                style={{ width: `${downloadProgress}%` }}
              />
            </div>
          </div>
        );
      }

      if (state === "ready") {
        return (
          <div>
            <p className="text-sm font-medium mb-1">Update ready</p>
            <p className="text-xs text-muted-foreground mb-3">
              Restart to apply v{updateVersion}
            </p>
            <Button size="sm" className="w-full" onClick={installAndRestart}>
              Restart Now
            </Button>
          </div>
        );
      }

      // update-available
      return (
        <div>
          <p className="text-sm font-medium mb-1">Update available</p>
          <p className="text-xs text-muted-foreground mb-3">
            Codemux v{updateVersion} is ready
          </p>
          <div className="flex gap-2">
            {canAutoUpdate ? (
              <Button size="sm" className="flex-1" onClick={startDownload}>
                Install &amp; Restart
              </Button>
            ) : (
              <Button
                size="sm"
                className="flex-1"
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
        </div>
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
