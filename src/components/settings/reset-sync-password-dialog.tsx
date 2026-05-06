// Multi-step "Reset sync password" dialog (Stage 4 of Step 10).
//
// The user reaches this dialog via the "Forgot your sync password?"
// link in Settings → Sync. Reset is destructive — every encrypted
// skill on the server gets wiped because the new password produces
// a different encryption_key and the old ciphertext becomes
// undecryptable. The dialog enforces the safety net the research
// doc §4.3 prescribed: encourage a local export first, then make
// the destructive step explicit.
//
// Steps:
//
//   1. Warn — explain what reset will do; offer "export first" or
//      "skip backup."
//   2a. Export — OS save-dialog → write file → confirm complete.
//   2b. Skip — extra "are you sure?" gate (small typed-confirmation
//       moment, here a checkbox).
//   3. Confirm wipe + email — show what's about to happen, click
//      to fire `wipeRemoteSkillsForReset`.
//   4. Done — instructions for finishing the reset via the email
//      link, then importing the backup back in.
//
// The actual password reset happens in the user's email client +
// the codemux-api reset-password page (Stage 1's
// `api/src/reset-password/`). This dialog only handles the parts
// that have to happen inside the Tauri app: export, server-skill
// wipe, local-key wipe, email trigger.

import { useState } from "react";
import { AlertTriangle, CheckCircle2, FileDown, KeyRound, Loader2 } from "lucide-react";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { useAuthStore } from "@/stores/auth-store";
import {
  exportSkillsToFile,
  getExportRecommendedFilename,
  pickSaveFileDialog,
  wipeRemoteSkillsForReset,
} from "@/tauri/commands";

type Step = "warn" | "export-running" | "exported" | "skip-confirm" | "wiping" | "done" | "error";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function ResetSyncPasswordDialog({ open, onOpenChange }: Props) {
  const userEmail = useAuthStore((s) => s.user?.email ?? "");

  const [step, setStep] = useState<Step>("warn");
  const [exportPath, setExportPath] = useState<string | null>(null);
  const [exportSkillCount, setExportSkillCount] = useState(0);
  const [skipAcknowledged, setSkipAcknowledged] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const reset = () => {
    setStep("warn");
    setExportPath(null);
    setExportSkillCount(0);
    setSkipAcknowledged(false);
    setError(null);
  };

  const close = () => {
    onOpenChange(false);
    // Defer reset until the close animation finishes; otherwise
    // the user briefly sees the warning step flash back during the
    // dialog's exit transition.
    setTimeout(reset, 200);
  };

  const handleExport = async () => {
    setError(null);
    try {
      const defaultFilename = await getExportRecommendedFilename();
      const path = await pickSaveFileDialog({
        title: "Export skills",
        defaultFilename,
        filterName: "JSON",
        filterExtensions: ["json"],
      });
      if (!path) return; // user cancelled, stay on warn step
      setStep("export-running");
      const summary = await exportSkillsToFile(path);
      setExportPath(summary.path);
      setExportSkillCount(summary.skillCount);
      setStep("exported");
    } catch (err) {
      setError(String(err));
      setStep("error");
    }
  };

  const handleWipe = async () => {
    setError(null);
    setStep("wiping");
    try {
      await wipeRemoteSkillsForReset();
      setStep("done");
    } catch (err) {
      setError(String(err));
      setStep("error");
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[480px]">
        {step === "warn" && (
          <>
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <AlertTriangle className="h-5 w-5 text-destructive" />
                Reset sync password
              </DialogTitle>
              <DialogDescription>
                Resetting will <strong>permanently delete every synced skill from
                the server</strong>. The encrypted blobs are tied to your current
                password — they cannot be recovered without it.
              </DialogDescription>
            </DialogHeader>
            <p className="text-xs text-muted-foreground">
              Before continuing, export your skills to a local backup. After you
              finish the reset, you can import the backup to restore them under
              your new password.
            </p>
            <DialogFooter className="gap-2 sm:justify-between">
              <Button variant="ghost" onClick={close}>
                Cancel
              </Button>
              <div className="flex gap-2">
                <Button variant="outline" onClick={() => setStep("skip-confirm")}>
                  I don't need a backup
                </Button>
                <Button onClick={handleExport}>
                  <FileDown className="mr-2 h-4 w-4" />
                  Export skills first
                </Button>
              </div>
            </DialogFooter>
          </>
        )}

        {step === "export-running" && (
          <>
            <DialogHeader>
              <DialogTitle>Exporting skills…</DialogTitle>
              <DialogDescription>
                Decrypting and writing your backup file. This usually takes a
                few seconds.
              </DialogDescription>
            </DialogHeader>
            <div className="flex justify-center py-6">
              <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
            </div>
          </>
        )}

        {step === "exported" && (
          <>
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <CheckCircle2 className="h-5 w-5 text-emerald-500" />
                Backup saved
              </DialogTitle>
              <DialogDescription>
                Saved {exportSkillCount} skill{exportSkillCount === 1 ? "" : "s"} to
                a local file.
              </DialogDescription>
            </DialogHeader>
            <div className="rounded-md border border-border bg-muted/30 p-3 font-mono text-xs">
              {exportPath}
            </div>
            <p className="text-xs text-muted-foreground">
              You'll be able to import this file from Settings → Sync after you
              set a new password.
            </p>
            <DialogFooter className="gap-2 sm:justify-between">
              <Button variant="ghost" onClick={close}>
                Cancel
              </Button>
              <Button variant="destructive" onClick={handleWipe}>
                Continue with reset
              </Button>
            </DialogFooter>
          </>
        )}

        {step === "skip-confirm" && (
          <>
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <AlertTriangle className="h-5 w-5 text-destructive" />
                Continue without a backup?
              </DialogTitle>
              <DialogDescription>
                Without a backup, every synced skill is gone the moment we wipe
                the server. This <strong>cannot</strong> be undone — there is no
                way for anyone (including the Codemux operator) to recover your
                skills after reset.
              </DialogDescription>
            </DialogHeader>
            <label className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-2 text-xs">
              <input
                type="checkbox"
                checked={skipAcknowledged}
                onChange={(e) => setSkipAcknowledged(e.target.checked)}
                className="mt-0.5 h-4 w-4 shrink-0 cursor-pointer accent-destructive"
              />
              <span>
                I understand my synced skills will be permanently deleted and
                cannot be recovered.
              </span>
            </label>
            <DialogFooter className="gap-2 sm:justify-between">
              <Button variant="ghost" onClick={() => setStep("warn")}>
                Back
              </Button>
              <Button
                variant="destructive"
                disabled={!skipAcknowledged}
                onClick={handleWipe}
              >
                Wipe my skills and reset
              </Button>
            </DialogFooter>
          </>
        )}

        {step === "wiping" && (
          <>
            <DialogHeader>
              <DialogTitle>Wiping server data…</DialogTitle>
              <DialogDescription>
                Deleting your encrypted skills from the server, clearing your
                local sync key, and emailing you a password-reset link.
              </DialogDescription>
            </DialogHeader>
            <div className="flex justify-center py-6">
              <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
            </div>
          </>
        )}

        {step === "done" && (
          <>
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <KeyRound className="h-5 w-5" />
                Check your email
              </DialogTitle>
              <DialogDescription>
                We sent a password-reset link to <strong>{userEmail}</strong>.
                Open it and choose a new password.
              </DialogDescription>
            </DialogHeader>
            <ol className="space-y-2 text-xs text-muted-foreground">
              <li>1. Click the link in your email.</li>
              <li>2. Set a new password.</li>
              <li>3. Come back here and sign in with the new password.</li>
              {exportPath && (
                <li>
                  4. Open Settings → Sync and click <strong>Import skills from
                  backup</strong> to restore from your export.
                </li>
              )}
            </ol>
            <DialogFooter>
              <Button onClick={close}>Got it</Button>
            </DialogFooter>
          </>
        )}

        {step === "error" && (
          <>
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <AlertTriangle className="h-5 w-5 text-destructive" />
                Something went wrong
              </DialogTitle>
              <DialogDescription>
                {error ??
                  "An unexpected error happened. No changes were applied; you can try again."}
              </DialogDescription>
            </DialogHeader>
            <DialogFooter className="gap-2 sm:justify-between">
              <Button variant="ghost" onClick={close}>
                Close
              </Button>
              <Button onClick={() => setStep("warn")}>Try again</Button>
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}
