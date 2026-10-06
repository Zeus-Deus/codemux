import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { appLogDir, join } from "@tauri-apps/api/path";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { Check, Copy, ExternalLink } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { canAutoUpdateFormat } from "@/hooks/use-update-checker";
import { copyToClipboard, COPY_FAILED_MESSAGE } from "@/lib/clipboard";
import { collectPerformanceDiagnostics } from "@/lib/perf/performance-diagnostics";
import { toast } from "@/lib/toast";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { useUpdateStatusStore } from "@/stores/update-status-store";
import { getPackageFormat } from "@/tauri/commands";
import { SectionHeader, SettingRow } from "./settings-primitives";

const RELEASES_URL = "https://github.com/Zeus-Deus/codemux/releases";
/** tauri-plugin-log's file in the app log dir; see `app_logs.rs`. */
const LOG_FILE_NAME = "codemux.log";

const PACKAGE_LABELS: Record<string, string> = {
  appimage: "AppImage",
  nsis: "Windows installer",
};

/** How this build was installed, in words. Anything the updater cannot
 *  replace in place (deb, rpm, AUR) reports as "other". */
function packageLabel(format: string | null): string | null {
  if (import.meta.env.DEV) return "Development build";
  if (format === null) return null;
  return PACKAGE_LABELS[format.toLowerCase()] ?? "System package";
}

/**
 * Settings → About. The one place to find which build is running, whether it
 * is current, and what to attach to a bug report — plus the reset that the
 * synced-settings store has always offered but nothing exposed.
 */
export function AboutSection() {
  const remote = isRemoteClient();
  const [version, setVersion] = useState<string | null>(null);
  const [format, setFormat] = useState<string | null>(null);

  useEffect(() => {
    getVersion().then(setVersion).catch(() => setVersion(null));
    if (remote) return;
    getPackageFormat().then(setFormat).catch(() => setFormat(null));
  }, [remote]);

  const channel = packageLabel(format);
  const releaseNotesUrl =
    version && !import.meta.env.DEV ? `${RELEASES_URL}/tag/v${version}` : RELEASES_URL;

  return (
    <div>
      <SectionHeader
        title="About"
        description="Which build you are running, whether it is current, and what to attach when you report a problem."
      />
      <div className="space-y-1">
        <SettingRow
          label="Version"
          description={[version && `Codemux v${version}`, channel].filter(Boolean).join(" · ")}
        >
          <Button
            variant="ghost"
            size="sm"
            className="text-muted-foreground"
            onClick={() => void openUrl(releaseNotesUrl).catch(console.error)}
          >
            Release notes
            <ExternalLink className="size-3.5" />
          </Button>
        </SettingRow>
        <Separator />
        <UpdatesRow canAutoUpdate={canAutoUpdateFormat((format ?? "").toLowerCase())} />
        <Separator />
        <DiagnosticsRow />
        {!remote && (
          <>
            <Separator />
            <SettingRow
              label="Logs"
              description="Codemux keeps one rotating log file. Attach it to a bug report."
            >
              <Button variant="outline" size="sm" onClick={() => void revealLogFile()}>
                Show log file
              </Button>
            </SettingRow>
          </>
        )}
        <Separator />
        <ResetSettingsRow />
      </div>
    </div>
  );
}

async function revealLogFile() {
  try {
    await revealItemInDir(await join(await appLogDir(), LOG_FILE_NAME));
  } catch (error) {
    toast.error(`Couldn't open the log folder: ${String(error)}`);
  }
}

/** Update status and the one action that moves it forward. Reads the mirror
 *  the app's single update checker publishes, so a click here drives the same
 *  flow as the update toast. */
function UpdatesRow({ canAutoUpdate }: { canAutoUpdate: boolean }) {
  const state = useUpdateStatusStore((s) => s.state);
  const updateVersion = useUpdateStatusStore((s) => s.updateVersion);
  const progress = useUpdateStatusStore((s) => s.downloadProgress);
  const isRemote = useUpdateStatusStore((s) => s.isRemote);
  const lastCheck = useUpdateStatusStore((s) => s.lastCheck);
  const checkNow = useUpdateStatusStore((s) => s.checkNow);
  const startDownload = useUpdateStatusStore((s) => s.startDownload);
  const installAndRestart = useUpdateStatusStore((s) => s.installAndRestart);
  const requestDesktopUpdate = useUpdateStatusStore((s) => s.requestDesktopUpdate);

  let description: string;
  let action: { label: string; run: (() => void) | null } | null = null;
  switch (state) {
    case "checking":
      description = "Checking for updates…";
      action = { label: "Checking…", run: null };
      break;
    case "update-available":
      description = `Version ${updateVersion ?? "?"} is available.`;
      if (isRemote) {
        action = { label: "Update desktop", run: requestDesktopUpdate };
      } else if (canAutoUpdate) {
        action = { label: "Download update", run: startDownload };
      } else {
        // deb/rpm/AUR installs are replaced by their package manager, so the
        // only honest action is the release page.
        action = {
          label: "Open release page",
          run: () =>
            void openUrl(`${RELEASES_URL}/tag/v${updateVersion}`).catch(console.error),
        };
      }
      break;
    case "downloading":
      description = `Downloading the update… ${progress}%`;
      action = { label: "Downloading…", run: null };
      break;
    case "ready":
      description = "The update is installed. Restart Codemux to finish.";
      action = { label: "Restart to update", run: installAndRestart };
      break;
    case "error":
      description = "The last update failed. Check again or download it from the release page.";
      action = { label: "Check for updates", run: checkNow };
      break;
    default:
      if (isRemote) {
        description = "The desktop app checks for updates and tells this browser when one is ready.";
      } else if (lastCheck && !lastCheck.ok) {
        description = "Couldn't reach the update server. Check your connection and try again.";
      } else if (lastCheck) {
        description = `You're on the latest version. Checked at ${new Date(lastCheck.at).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}.`;
      } else {
        description = "Codemux checks for updates in the background every few hours.";
      }
      action = isRemote ? null : { label: "Check for updates", run: checkNow };
  }

  return (
    <SettingRow label="Updates" description={description}>
      {action && (
        <Button
          variant="outline"
          size="sm"
          disabled={!action.run}
          onClick={() => action.run?.()}
        >
          {action.label}
        </Button>
      )}
    </SettingRow>
  );
}

function DiagnosticsRow() {
  const [copying, setCopying] = useState(false);
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    if (copying) return;
    setCopying(true);
    try {
      const report = await collectPerformanceDiagnostics();
      if (!(await copyToClipboard(JSON.stringify(report, null, 2)))) {
        toast.error(COPY_FAILED_MESSAGE);
        return;
      }
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1_400);
      toast.success("Performance diagnostics copied");
    } catch (error) {
      toast.error(`Couldn't collect performance diagnostics: ${String(error)}`);
    } finally {
      setCopying(false);
    }
  };

  return (
    <SettingRow
      label="Performance diagnostics"
      description="Copy bounded startup, workspace-switch, renderer, payload-size and native timing summaries. Paths, titles, messages and IDs are excluded."
    >
      <Button variant="outline" size="sm" disabled={copying} onClick={() => void copy()}>
        {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
        {copied ? "Copied" : copying ? "Collecting…" : "Copy diagnostics"}
      </Button>
    </SettingRow>
  );
}

function ResetSettingsRow() {
  const [confirming, setConfirming] = useState(false);
  const resetSettings = useSyncedSettingsStore((s) => s.resetSettings);

  const reset = () => {
    setConfirming(false);
    void resetSettings().then(() => toast.success("Settings reset to defaults"));
  };

  return (
    <>
      <SettingRow
        label="Reset settings"
        description="Return every synced setting (appearance, editor, terminal, Git, shortcuts, notifications, browser, agent chat and session restore) to its default. Signed in, this applies on every device. Preferences kept only on this device, such as density and sidebar options, stay as they are."
      >
        <Button variant="destructive" size="sm" onClick={() => setConfirming(true)}>
          Reset…
        </Button>
      </SettingRow>
      <AlertDialog open={confirming} onOpenChange={setConfirming}>
        {/* Escape closes this dialog only, not Settings behind it. */}
        <AlertDialogContent onEscapeKeyDown={(event) => event.stopPropagation()}>
          <AlertDialogHeader>
            <AlertDialogTitle>Reset all synced settings?</AlertDialogTitle>
            <AlertDialogDescription>
              Your theme, fonts, keyboard shortcuts and the other synced settings go back to their
              defaults. Projects, presets, workspaces and chats are not touched. This can't be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction variant="destructive" onClick={reset}>
              Reset settings
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
