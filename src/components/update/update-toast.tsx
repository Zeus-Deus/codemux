import { useUpdateChecker } from "@/hooks/use-update-checker";
import { Button } from "@/components/ui/button";
import { openUrl } from "@tauri-apps/plugin-opener";
import { cn } from "@/lib/utils";

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

  const visible =
    !dismissed &&
    (state === "update-available" || state === "downloading" || state === "ready");

  if (!visible) return null;

  return (
    <div
      className={cn(
        "fixed bottom-4 right-4 z-50",
        "animate-in slide-in-from-bottom-2 duration-[160ms] ease-out",
      )}
    >
      <div className="bg-card border border-border rounded-lg shadow-lg p-4 w-80">
        {state === "downloading" && (
          <>
            <p className="text-sm font-medium text-foreground mb-2">
              Downloading update...
            </p>
            <div className="bg-muted rounded-full h-1.5 overflow-hidden">
              <div
                className="bg-primary h-full rounded-full transition-all duration-200"
                style={{ width: `${downloadProgress}%` }}
              />
            </div>
          </>
        )}

        {state === "ready" && (
          <>
            <p className="text-sm font-medium text-foreground mb-1">
              Update ready
            </p>
            <p className="text-xs text-muted-foreground mb-3">
              Restart to apply v{updateVersion}
            </p>
            <Button size="sm" className="w-full" onClick={installAndRestart}>
              Restart Now
            </Button>
          </>
        )}

        {state === "update-available" && (
          <>
            <p className="text-sm font-medium text-foreground mb-1">
              Update available
            </p>
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
          </>
        )}
      </div>
    </div>
  );
}
