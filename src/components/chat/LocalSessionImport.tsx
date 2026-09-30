import { useEffect, useRef, useState } from "react";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { useMobileLayout } from "@/hooks/use-mobile-layout";
import { useFeatureFlags } from "@/stores/feature-flags";
import { Button } from "@/components/ui/button";
import {
  Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle,
} from "@/components/ui/dialog";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { selectActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { containsPane } from "@/remote/pane-selection";
import { useUIStore } from "@/stores/ui-store";
import {
  agentChatScanLocalSessions, agentChatImportLocalSessions,
  agentChatOpenSearchResult, getAppState, type LocalChatSession, type LocalChatImportResult,
} from "@/tauri/commands";

export function LocalSessionImport() {
  const open = useUIStore((s) => s.showLocalSessionImport);
  const enabled = useFeatureFlags((s) => s.enableAgentChat);
  const mobile = useMobileLayout();
  // Unmount on close: each opt-in starts fresh, without stale discovery data.
  return open && enabled && !mobile && !isRemoteClient() ? <ImportDialog /> : null;
}

function ImportDialog() {
  const setOpen = useUIStore((s) => s.setShowLocalSessionImport);
  const [sessions, setSessions] = useState<LocalChatSession[] | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [warnings, setWarnings] = useState<string[]>([]);
  const [result, setResult] = useState<LocalChatImportResult | null>(null);
  const [busy, setBusy] = useState<"scan" | "import" | null>(null);
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inFlight = useRef(false);
  const mounted = useRef(true);
  const available = sessions?.filter((s) => !s.already_imported) ?? [];
  const dismissalLocked = busy === "import" || opening;

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  function close() {
    if (dismissalLocked) return;
    mounted.current = false;
    setOpen(false);
  }

  async function scan() {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy("scan");
    setError(null);
    try {
      const found = await agentChatScanLocalSessions();
      if (!mounted.current) return;
      setSessions(found.sessions);
      setWarnings(found.warnings);
      setSelected([]);
    } catch (err) {
      if (mounted.current) setError(String(err instanceof Error ? err.message : err));
    } finally {
      inFlight.current = false;
      if (mounted.current) setBusy(null);
    }
  }

  async function importSelected() {
    if (inFlight.current || selected.length === 0) return;
    inFlight.current = true;
    setBusy("import");
    setError(null);
    try {
      const imported = await agentChatImportLocalSessions(selected);
      if (!mounted.current) return;
      setResult(imported);
      setWarnings(imported.warnings);
    } catch (err) {
      if (mounted.current) setError(String(err instanceof Error ? err.message : err));
    } finally {
      inFlight.current = false;
      if (mounted.current) setBusy(null);
    }
  }

  async function openImported(threadId: string) {
    if (inFlight.current) return;
    inFlight.current = true;
    setOpening(true);
    setError(null);
    try {
      const opened = await agentChatOpenSearchResult(threadId);
      if (!mounted.current) return;
      // IPC can return before the workspace/pane snapshot is delivered. Keep
      // the draft overlay until a real read-back makes the returned pane
      // renderable; clearing it earlier lets the empty-state hook reselect
      // (or create) a Home draft that then masks the imported transcript.
      const snapshot = await getAppState();
      if (!mounted.current) return;
      useAppStore.getState().setAppState(snapshot);
      // Inspect the reconciled store, not the fetched snapshot: a newer event
      // or optimistic navigation may have superseded this read while in flight.
      const state = useAppStore.getState();
      const workspace = state.appState?.workspaces.find((w) => w.workspace_id === opened.workspace_id);
      const surface = workspace?.surfaces.find((s) => s.surface_id === workspace.active_surface_id);
      if (selectActiveWorkspaceId(state) !== opened.workspace_id ||
          !surface || surface.active_pane_id !== opened.pane_id ||
          !containsPane(surface.root, opened.pane_id)) {
        throw new Error("Conversation selection is not available yet. Please try opening it again.");
      }
      // Navigation is separate from import. Preserve all unsent draft records.
      useChatDraftStore.getState().setActiveDraft(null);
      useUIStore.getState().setShowSettings(false);
      mounted.current = false;
      setOpen(false);
    } catch (err) {
      if (mounted.current) {
        setError(`Imported successfully, but could not open the conversation: ${err instanceof Error ? err.message : String(err)}`);
      }
    } finally {
      inFlight.current = false;
      if (mounted.current) setOpening(false);
    }
  }

  return (
    <Dialog open onOpenChange={(next) => { if (!next) close(); }}>
      <DialogContent
        showCloseButton={!dismissalLocked}
        onEscapeKeyDown={(event) => { if (dismissalLocked) event.preventDefault(); }}
        onPointerDownOutside={(event) => { if (dismissalLocked) event.preventDefault(); }}
        className="sm:max-w-2xl max-h-[85vh] overflow-y-auto"
      >
        <DialogHeader>
          <DialogTitle>Import recent chats</DialogTitle>
          <DialogDescription>
            Review recent Claude Code and Codex conversations from this computer.
            Nothing is scanned until you choose Find recent chats.
          </DialogDescription>
        </DialogHeader>
        {sessions === null && (
          <div className="space-y-2 text-body text-muted-foreground">
            <p>Selected user and assistant text is copied to your local Codemux database. Original files remain unchanged.</p>
            <p>Tool output, system prompts, images, and hidden reasoning are excluded. User and assistant text may contain secrets — review your selection.</p>
            <p>The import itself does not upload or run an agent. Copies are read-only, not provider-native sessions you can resume.</p>
          </div>
        )}
        {error && <p role="alert" className="text-body text-destructive">{error}</p>}
        {busy && (
          <p role="status">
            {busy === "scan" ? "Finding recent chats…" : "Importing selected conversations…"}
          </p>
        )}
        {sessions?.length === 0 && <p>No recent chats found.</p>}
        {warnings.map((warning, i) => (
          <p key={i} className="text-label text-muted-foreground">{warning}</p>
        ))}
        {result ? (
          <div role="status" className="space-y-3">
            <p>Imported {result.imported.length} conversation{result.imported.length === 1 ? "" : "s"}.</p>
            {result.skipped > 0 && (
              <p>Skipped {result.skipped} conversation{result.skipped === 1 ? "" : "s"}.</p>
            )}
            <div className="flex flex-col items-start gap-2">
              {result.imported.map((entry) => (
                <Button
                  key={entry.source_id}
                  variant="secondary"
                  disabled={opening}
                  className="max-w-full whitespace-normal text-left"
                  onClick={() => void openImported(entry.thread_id)}
                >
                  Open {sessions?.find((session) => session.source_id === entry.source_id)?.title ?? "conversation"}
                </Button>
              ))}
            </div>
          </div>
        ) : sessions && (
          <div className="space-y-2">
            {available.length > 0 && (
              <label className="flex items-center gap-3 px-3 py-2 text-body">
                <input
                  type="checkbox"
                  disabled={busy !== null}
                  checked={selected.length === available.length}
                  onChange={(event) => setSelected(event.target.checked ? available.map((s) => s.source_id) : [])}
                />
                Select all available chats
              </label>
            )}
            {sessions.map((session) => (
              <label key={session.source_id} className="flex items-start gap-3 rounded-lg border border-border p-3">
                <input
                  type="checkbox"
                  className="mt-1"
                  disabled={busy !== null || session.already_imported}
                  checked={selected.includes(session.source_id)}
                  onChange={(event) => setSelected((ids) => event.target.checked ? [...ids, session.source_id] : ids.filter((id) => id !== session.source_id))}
                />
                <span className="min-w-0 space-y-1">
                  <span className="block text-body font-medium break-words">{session.title}</span>
                  <span className="block text-label text-muted-foreground">
                    {session.provider === "claude" ? "Claude Code" : "Codex"} · {session.message_count} messages · {new Date(session.last_active_at).toLocaleDateString()}
                  </span>
                  <span className="block text-label text-muted-foreground break-all">{session.cwd}</span>
                  {session.already_imported && <span className="block text-label text-muted-foreground">Already imported</span>}
                </span>
              </label>
            ))}
          </div>
        )}
        <div className="flex justify-end gap-2">
          {result ? <Button disabled={opening} onClick={close}>Done</Button> : (
            <>
              <Button variant="ghost" disabled={busy === "import"} onClick={close}>Not now</Button>
              {(sessions === null || sessions.length === 0) ? (
                <Button disabled={busy !== null} onClick={() => void scan()}>
                  {busy === "scan" ? "Finding…" : error ? "Retry discovery" : sessions ? "Find again" : "Find recent chats"}
                </Button>
              ) : (
                <Button disabled={busy !== null || selected.length === 0} onClick={() => void importSelected()}>
                  {busy === "import" ? "Importing…" : `Import selected (${selected.length})`}
                </Button>
              )}
            </>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
