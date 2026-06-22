// Settings → Sync — inline section that drives skills sync.
//
// Skills sync is stored server-side (the same model Codemux's
// settings sync already uses): a signed-in user is sync-ready
// immediately, with no password to set up and no device-local key
// to repair. Single-sign-on users (GitHub) sync without ever
// creating a password.
//
// Two render states:
//   1. syncAvailable=true  → "Sync ready" status + export/import
//   2. syncAvailable=false → a hint to sign in (only reachable when
//      the session is still settling or signed out)

import { useState } from "react";
import { FileDown, FileUp, Loader2 } from "lucide-react";
import { useAuthStore } from "@/stores/auth-store";
import {
  exportSkillsToFile,
  getExportRecommendedFilename,
  importSkillsFromFile,
  pickOpenFileDialog,
  pickSaveFileDialog,
} from "@/tauri/commands";
import { SyncStatusDisplay } from "./sync-status-display";

export function SyncSection() {
  const syncAvailable = useAuthStore((s) => s.syncAvailable);

  if (syncAvailable) {
    return <SyncReadyRow />;
  }

  return (
    <p className="rounded-md border border-border bg-muted/30 p-3 text-xs text-muted-foreground">
      Sign in to sync your skills across devices.
    </p>
  );
}

function SyncReadyRow() {
  const [busy, setBusy] = useState<"export" | "import" | null>(null);
  const [feedback, setFeedback] = useState<string | null>(null);

  const handleExport = async () => {
    setFeedback(null);
    setBusy("export");
    try {
      const defaultFilename = await getExportRecommendedFilename();
      const path = await pickSaveFileDialog({
        title: "Export skills",
        defaultFilename,
        filterName: "JSON",
        filterExtensions: ["json"],
      });
      if (!path) return;
      const summary = await exportSkillsToFile(path);
      setFeedback(
        `Exported ${summary.skillCount} skill${summary.skillCount === 1 ? "" : "s"} to ${summary.path}`,
      );
    } catch (err) {
      setFeedback(`Export failed: ${err}`);
    } finally {
      setBusy(null);
    }
  };

  const handleImport = async () => {
    setFeedback(null);
    setBusy("import");
    try {
      const path = await pickOpenFileDialog({
        title: "Import skills from backup",
        filterName: "JSON",
        filterExtensions: ["json"],
      });
      if (!path) return;
      const summary = await importSkillsFromFile(path);
      const warn = summary.mismatchedEmail
        ? " (note: backup belongs to a different account)"
        : "";
      setFeedback(
        `Re-pushed ${summary.queuedCount} skill${summary.queuedCount === 1 ? "" : "s"}${
          summary.failedCount > 0 ? `, ${summary.failedCount} failed` : ""
        }${warn}`,
      );
    } catch (err) {
      setFeedback(`Import failed: ${err}`);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="space-y-3">
      <div className="rounded-md border border-border bg-muted/30 p-3">
        <SyncStatusDisplay />
        <p className="mt-3 text-xs text-muted-foreground">
          Your skills sync to your Codemux account so they follow you
          across devices. They're stored on Codemux servers, encrypted
          at rest.
        </p>
      </div>

      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          onClick={handleExport}
          disabled={busy !== null}
          className="inline-flex items-center gap-2 rounded-md border border-border bg-background px-3 py-1.5 text-xs font-medium hover:bg-muted disabled:cursor-not-allowed disabled:opacity-40"
        >
          {busy === "export" ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <FileDown className="h-3.5 w-3.5" />
          )}
          Export skills locally
        </button>
        <button
          type="button"
          onClick={handleImport}
          disabled={busy !== null}
          className="inline-flex items-center gap-2 rounded-md border border-border bg-background px-3 py-1.5 text-xs font-medium hover:bg-muted disabled:cursor-not-allowed disabled:opacity-40"
        >
          {busy === "import" ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <FileUp className="h-3.5 w-3.5" />
          )}
          Import skills from backup
        </button>
      </div>

      {feedback && (
        <p className="text-xs text-muted-foreground" role="status">
          {feedback}
        </p>
      )}
    </div>
  );
}
