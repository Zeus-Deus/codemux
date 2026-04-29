// Settings → Sync — inline section that drives skills sync setup and
// repair. Stage 2 of Step 10. Mirrors Vexis's `SyncSection` /
// `SetupSyncPasswordForm` / `ProvidePasswordForm` pattern (see
// `~/projects/vexis/src/components/settings/settings-view.tsx`),
// adapted for skills (instead of voice transcriptions/dictionary).
//
// Three render states based on `(syncAvailable, authMethod)`:
//
//   1. syncAvailable=true                      → "Sync ready" status
//   2. syncAvailable=false, authMethod=github  → SetupSyncPasswordForm
//   3. syncAvailable=false, authMethod≠github  → ProvidePasswordForm
//
// State (3) is the email-user repair flow: their local
// `sync-key.enc` was lost (manual deletion, /etc/machine-id rotated,
// machine swapped) and they need to re-derive from the same
// password they used at signin. No server interaction; wrong
// password is detected lazily by Stage 3's first sync attempt.
//
// Setup is intentionally NOT a forced modal at login. Skills sync
// is opt-in — new OAuth users land in the app fully usable, and
// only see this form if they navigate to Settings → Account.

import { useState, type FormEvent } from "react";
import {
  FileDown,
  FileUp,
  KeyRound,
  Loader2,
  Lock,
  ShieldAlert,
} from "lucide-react";
import { useAuthStore } from "@/stores/auth-store";
import {
  exportSkillsToFile,
  getExportRecommendedFilename,
  importSkillsFromFile,
  pickOpenFileDialog,
  pickSaveFileDialog,
  providePasswordForSync,
  setupSyncPassword,
} from "@/tauri/commands";
import { ResetSyncPasswordDialog } from "./reset-sync-password-dialog";
import { SyncStatusDisplay } from "./sync-status-display";

const MIN_LEN = 8;

function isPasswordShapeOk(pw: string): boolean {
  if (pw.length < MIN_LEN) return false;
  // Match Vexis: cheap structural check, no zxcvbn dep. Argon2id
  // does the actual cost-stretching; the UI just stops obvious
  // junk like "aaaaaaaa".
  const hasLetter = /[A-Za-z]/.test(pw);
  const hasDigit = /\d/.test(pw);
  return hasLetter && hasDigit;
}

export function SyncSection() {
  const syncAvailable = useAuthStore((s) => s.syncAvailable);
  const authMethod = useAuthStore((s) => s.authMethod);
  const setSyncStatus = useAuthStore((s) => s.setSyncStatus);

  if (syncAvailable) {
    return <SyncReadyRow />;
  }

  if (authMethod === "github") {
    return <SetupSyncPasswordForm onApply={setSyncStatus} />;
  }

  return <ProvidePasswordForm onApply={setSyncStatus} />;
}

function SyncReadyRow() {
  const [busy, setBusy] = useState<"export" | "import" | null>(null);
  const [resetOpen, setResetOpen] = useState(false);
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
          Your skills are end-to-end encrypted with a key derived
          from your password. Codemux servers store ciphertext only —
          nobody else can read your skills.
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

      <button
        type="button"
        onClick={() => setResetOpen(true)}
        className="inline-flex items-center gap-1 text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
      >
        <ShieldAlert className="h-3 w-3" />
        Forgot your sync password?
      </button>

      <ResetSyncPasswordDialog open={resetOpen} onOpenChange={setResetOpen} />
    </div>
  );
}

function SetupSyncPasswordForm({
  onApply,
}: {
  onApply: (status: { syncAvailable: boolean; authMethod: "email" | "github" | null }) => void;
}) {
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  // Required acknowledgment that the user understands forgetting
  // the password is unrecoverable. Borrowed verbatim from Vexis;
  // every E2E app (Bitwarden, Proton, 1Password) has this gate.
  const [ackKeyLoss, setAckKeyLoss] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const passwordsMatch = password === confirm;
  const shapeOk = isPasswordShapeOk(password);
  const canSubmit = passwordsMatch && shapeOk && ackKeyLoss && !busy;

  const handleSubmit = async (ev: FormEvent) => {
    ev.preventDefault();
    setError(null);
    if (!shapeOk) {
      setError(
        `Password must be at least ${MIN_LEN} characters with at least one letter and one digit.`,
      );
      return;
    }
    if (!passwordsMatch) {
      setError("Passwords don't match.");
      return;
    }
    if (!ackKeyLoss) {
      setError("Please acknowledge the recovery warning before continuing.");
      return;
    }
    setBusy(true);
    try {
      const status = await setupSyncPassword(password);
      setPassword("");
      setConfirm("");
      setAckKeyLoss(false);
      onApply(status);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="flex flex-col gap-3 rounded-md border border-border bg-background p-3"
      onSubmit={handleSubmit}
    >
      <div className="flex items-center gap-2 text-sm font-medium">
        <Lock className="h-4 w-4" />
        Set up skills sync
      </div>
      <p className="text-xs text-muted-foreground">
        Choose a password to encrypt your skills end-to-end. The
        server stores opaque ciphertext only — your password and
        encryption key never leave this device.
      </p>
      <label className="flex flex-col gap-1 text-xs font-medium text-muted-foreground">
        New password
        <input
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          disabled={busy}
          autoComplete="new-password"
          className="w-full rounded-md border border-border bg-background px-3 py-2 text-sm text-foreground outline-none focus:ring-2 focus:ring-foreground/30 disabled:opacity-40"
        />
      </label>
      <label className="flex flex-col gap-1 text-xs font-medium text-muted-foreground">
        Confirm password
        <input
          type="password"
          value={confirm}
          onChange={(e) => setConfirm(e.target.value)}
          disabled={busy}
          autoComplete="new-password"
          className="w-full rounded-md border border-border bg-background px-3 py-2 text-sm text-foreground outline-none focus:ring-2 focus:ring-foreground/30 disabled:opacity-40"
        />
      </label>
      <label className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-2 text-xs text-foreground">
        <input
          type="checkbox"
          checked={ackKeyLoss}
          onChange={(e) => setAckKeyLoss(e.target.checked)}
          disabled={busy}
          className="mt-0.5 h-4 w-4 shrink-0 cursor-pointer accent-destructive"
        />
        <span>
          I understand that my password is the <strong>only</strong> key
          to my synced skills. If I forget it, my synced skills will be{" "}
          <strong>permanently unrecoverable</strong>. There is no way
          for anyone — including the Codemux operator — to reset it for
          me.
        </span>
      </label>
      <p className="text-xs text-muted-foreground">
        You can export your skills anytime from Settings → Sync as a
        backup, even after setup.
      </p>
      {error && (
        <p className="text-xs text-destructive" role="alert">
          {error}
        </p>
      )}
      <button
        type="submit"
        disabled={!canSubmit}
        className="inline-flex items-center justify-center gap-2 rounded-md bg-foreground px-3 py-2 text-sm font-medium text-background hover:bg-foreground/90 disabled:cursor-not-allowed disabled:opacity-40"
      >
        {busy ? (
          <Loader2 className="h-4 w-4 animate-spin" />
        ) : (
          <KeyRound className="h-4 w-4" />
        )}
        Set up sync
      </button>
    </form>
  );
}

function ProvidePasswordForm({
  onApply,
}: {
  onApply: (status: { syncAvailable: boolean; authMethod: "email" | "github" | null }) => void;
}) {
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSubmit = async (ev: FormEvent) => {
    ev.preventDefault();
    setError(null);
    if (password.length < MIN_LEN) {
      setError(`Password must be at least ${MIN_LEN} characters.`);
      return;
    }
    setBusy(true);
    try {
      const status = await providePasswordForSync(password);
      setPassword("");
      onApply(status);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="flex flex-col gap-3 rounded-md border border-border bg-background p-3"
      onSubmit={handleSubmit}
    >
      <div className="flex items-center gap-2 text-sm font-medium">
        <KeyRound className="h-4 w-4" />
        Re-enter your password to unlock sync
      </div>
      <p className="text-xs text-muted-foreground">
        Your local sync key isn't available on this device — it was
        either deleted, the machine identity changed, or this is a
        new install. Re-enter the password you used when you set up
        sync to derive the key locally. No server contact required.
      </p>
      <label className="flex flex-col gap-1 text-xs font-medium text-muted-foreground">
        Password
        <input
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          disabled={busy}
          autoComplete="current-password"
          className="w-full rounded-md border border-border bg-background px-3 py-2 text-sm text-foreground outline-none focus:ring-2 focus:ring-foreground/30 disabled:opacity-40"
        />
      </label>
      {error && (
        <p className="text-xs text-destructive" role="alert">
          {error}
        </p>
      )}
      <button
        type="submit"
        disabled={busy || password.length < MIN_LEN}
        className="inline-flex items-center justify-center gap-2 rounded-md bg-foreground px-3 py-2 text-sm font-medium text-background hover:bg-foreground/90 disabled:cursor-not-allowed disabled:opacity-40"
      >
        {busy ? (
          <Loader2 className="h-4 w-4 animate-spin" />
        ) : (
          <KeyRound className="h-4 w-4" />
        )}
        Unlock sync
      </button>
    </form>
  );
}
