import { useCallback, useEffect, useMemo, useState } from "react";

import {
  Loader2,
  Pencil,
  Plus,
  Server,
  Trash2,
  X,
  Check,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";
import {
  hostsAdd,
  hostsBootstrapInstall,
  hostsDelete,
  hostsList,
  hostsTestConnection,
  hostsUpdate,
  type HostTestResult,
  type HostView,
} from "@/tauri/commands";
import { useHostsStore } from "@/stores/hosts-store";

/**
 * Settings → Hosts (Step 2 of cloud-push).
 *
 * Mirrors the shape of superset-sh's `/settings/hosts` route:
 * sidebar listing on the left grouped by Online/Offline (today
 * everything sits in Offline because SSH transport ships in 2d),
 * detail pane on the right with name + SSH target + Test connection
 * + Remove. "Add host" lives at the bottom of the sidebar.
 *
 * SSH credentials are never part of any payload. Auth happens at the
 * OS level via the user's `~/.ssh/config`, agent, and known_hosts.
 *
 * Online/offline today is a placeholder — `hostsTestConnection`
 * returns a "not implemented yet" message in 2a. The component is
 * already structured around the eventual real probe.
 */
export function HostsSection() {
  const [hosts, setHosts] = useState<HostView[]>([]);
  const [loading, setLoading] = useState(true);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Add-host form draft. `null` means the form isn't open.
  const [draft, setDraft] = useState<{ name: string; ssh_target: string } | null>(
    null,
  );

  // Edit mode for an existing host's fields. Keyed by host id so we
  // can have at most one row in edit mode at a time.
  const [editingId, setEditingId] = useState<number | null>(null);
  const [editDraft, setEditDraft] = useState<{ name: string; ssh_target: string }>(
    { name: "", ssh_target: "" },
  );

  // Per-host connection-test results. Cleared on host edit/delete.
  const [testResults, setTestResults] = useState<Record<number, HostTestResult>>(
    {},
  );
  const [testingId, setTestingId] = useState<number | null>(null);
  const [installingId, setInstallingId] = useState<number | null>(null);

  const reload = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const fresh = await hostsList();
      setHosts(fresh);
      // Keep selection stable across reloads when possible.
      if (fresh.length > 0 && selectedId === null) {
        setSelectedId(fresh[0].id);
      } else if (fresh.length === 0) {
        setSelectedId(null);
      } else if (selectedId !== null && !fresh.find((h) => h.id === selectedId)) {
        setSelectedId(fresh[0]?.id ?? null);
      }
    } catch (err) {
      setError(typeof err === "string" ? err : String(err));
    } finally {
      setLoading(false);
    }
  }, [selectedId]);

  useEffect(() => {
    void reload();
    // Intentionally not depending on `reload` — we only want this on mount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const selected = useMemo(
    () => hosts.find((h) => h.id === selectedId) ?? null,
    [hosts, selectedId],
  );

  const handleAdd = useCallback(async () => {
    if (!draft) return;
    const name = draft.name.trim();
    const sshTarget = draft.ssh_target.trim();
    if (!name || !sshTarget) {
      setError("Host name and SSH target are both required.");
      return;
    }
    try {
      const created = await hostsAdd(name, sshTarget);
      setHosts((prev) => [...prev, created].sort(byNameInsensitive));
      setSelectedId(created.id);
      setDraft(null);
      setError(null);
      // Invalidate the shared store so other surfaces (DevicePicker,
      // workspace context menu submenus) see the new host immediately
      // without a per-component refetch.
      void useHostsStore.getState().refresh();
    } catch (err) {
      setError(typeof err === "string" ? err : String(err));
    }
  }, [draft]);

  const handleStartEdit = useCallback((host: HostView) => {
    setEditingId(host.id);
    setEditDraft({ name: host.name, ssh_target: host.ssh_target });
    // Clear stale test result — the connection test was for the
    // old target.
    setTestResults((prev) => {
      const next = { ...prev };
      delete next[host.id];
      return next;
    });
  }, []);

  const handleSaveEdit = useCallback(async () => {
    if (editingId === null) return;
    const name = editDraft.name.trim();
    const sshTarget = editDraft.ssh_target.trim();
    if (!name || !sshTarget) {
      setError("Host name and SSH target are both required.");
      return;
    }
    try {
      const updated = await hostsUpdate(editingId, name, sshTarget);
      setHosts((prev) =>
        prev.map((h) => (h.id === editingId ? updated : h)).sort(byNameInsensitive),
      );
      setEditingId(null);
      setError(null);
      void useHostsStore.getState().refresh();
    } catch (err) {
      setError(typeof err === "string" ? err : String(err));
    }
  }, [editingId, editDraft]);

  const handleCancelEdit = useCallback(() => {
    setEditingId(null);
  }, []);

  const handleDelete = useCallback(async (host: HostView) => {
    const confirmed = window.confirm(
      `Remove "${host.name}" from your hosts? Your SSH config and keys are not affected.`,
    );
    if (!confirmed) return;
    try {
      await hostsDelete(host.id);
      setHosts((prev) => prev.filter((h) => h.id !== host.id));
      setTestResults((prev) => {
        const next = { ...prev };
        delete next[host.id];
        return next;
      });
      if (selectedId === host.id) {
        setSelectedId(null);
      }
      void useHostsStore.getState().refresh();
    } catch (err) {
      setError(typeof err === "string" ? err : String(err));
    }
  }, [selectedId]);

  const handleTestConnection = useCallback(async (host: HostView) => {
    setTestingId(host.id);
    try {
      const result = await hostsTestConnection(host.id);
      setTestResults((prev) => ({ ...prev, [host.id]: result }));
    } catch (err) {
      setTestResults((prev) => ({
        ...prev,
        [host.id]: {
          ok: false,
          message: typeof err === "string" ? err : String(err),
        },
      }));
    } finally {
      setTestingId(null);
    }
  }, []);

  const handleInstallRemote = useCallback(
    async (host: HostView, uname: string) => {
      // The "always auto-install" preference (set via the checkbox
      // below) skips the consent prompt for power users. Stored in
      // localStorage so it persists per-device — installing the
      // helper is a per-device decision (different machines may
      // have different SSH key access).
      const autoInstall =
        localStorage.getItem("codemux.hosts.autoInstallRemote") === "1";
      const consented =
        autoInstall ||
        window.confirm(
          `Install codemux-remote on ${host.name}?\n\n` +
            `Codemux Remote is a small helper (~8 MB) that runs in your ` +
            `user account on the host and lets your laptop run agents ` +
            `there. No root access required. Source: github.com/Zeus-Deus/codemux\n\n` +
            `Tip: enable "Always install automatically" in Settings → Hosts ` +
            `to skip this prompt on new hosts.`,
        );
      if (!consented) return;
      setInstallingId(host.id);
      try {
        const result = await hostsBootstrapInstall(host.id, uname);
        // Surface the install result alongside the test result so the
        // user sees "installed" then can press Test again to verify.
        setTestResults((prev) => ({
          ...prev,
          [host.id]: {
            ok: result.ok,
            message: result.message,
            needs_install: !result.ok && prev[host.id]?.needs_install,
            uname: prev[host.id]?.uname ?? uname,
          },
        }));
      } catch (err) {
        setTestResults((prev) => ({
          ...prev,
          [host.id]: {
            ok: false,
            message: typeof err === "string" ? err : String(err),
          },
        }));
      } finally {
        setInstallingId(null);
      }
    },
    [],
  );

  if (loading) {
    return (
      <div className="flex items-center justify-center py-12 text-sm text-muted-foreground">
        <Loader2 className="mr-2 size-4 animate-spin" />
        Loading hosts…
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-[400px]">
      {/* Sidebar */}
      <div className="w-64 shrink-0 border-r border-border pr-3">
        <div className="mb-3 flex items-center justify-between">
          <h3 className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
            Hosts
          </h3>
          <span className="text-[10px] text-muted-foreground/60 tabular-nums">
            {hosts.length}
          </span>
        </div>

        {hosts.length === 0 && !draft && (
          <p className="rounded-md border border-dashed border-border p-3 text-xs text-muted-foreground">
            No remote hosts yet. Add one to push workspaces from your laptop to a
            server you can SSH into.
          </p>
        )}

        <ul className="space-y-0.5">
          {hosts.map((host) => {
            const result = testResults[host.id];
            const isOnline = result?.ok === true;
            return (
              <li key={host.id}>
                <button
                  type="button"
                  onClick={() => setSelectedId(host.id)}
                  className={cn(
                    "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm transition-colors",
                    "hover:bg-muted/50",
                    selectedId === host.id && "bg-muted",
                  )}
                >
                  <span
                    aria-hidden
                    className={cn(
                      "size-1.5 shrink-0 rounded-full",
                      isOnline ? "bg-emerald-500" : "bg-muted-foreground/40",
                    )}
                  />
                  <span className="min-w-0 flex-1 truncate">{host.name}</span>
                  {host.dirty && (
                    <span
                      title="Pending sync"
                      className="text-[10px] text-amber-500"
                    >
                      •
                    </span>
                  )}
                </button>
              </li>
            );
          })}
        </ul>

        <div className="mt-3 border-t border-border pt-3">
          {draft ? (
            <div className="space-y-2 rounded-md border border-border bg-muted/30 p-3">
              <div className="space-y-1">
                <Label htmlFor="host-add-name" className="text-xs">
                  Name
                </Label>
                <Input
                  id="host-add-name"
                  placeholder="homelab"
                  value={draft.name}
                  onChange={(e) =>
                    setDraft({ ...draft, name: e.target.value })
                  }
                  autoFocus
                />
              </div>
              <div className="space-y-1">
                <Label htmlFor="host-add-target" className="text-xs">
                  SSH target
                </Label>
                <Input
                  id="host-add-target"
                  placeholder="zeus@10.0.0.5"
                  value={draft.ssh_target}
                  onChange={(e) =>
                    setDraft({ ...draft, ssh_target: e.target.value })
                  }
                />
                <p className="text-[10px] text-muted-foreground">
                  Anything `ssh` accepts. Your keys + config in `~/.ssh/` are
                  used as-is.
                </p>
              </div>
              <div className="flex justify-end gap-2 pt-1">
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() => {
                    setDraft(null);
                    setError(null);
                  }}
                >
                  Cancel
                </Button>
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  onClick={handleAdd}
                >
                  Add
                </Button>
              </div>
            </div>
          ) : (
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="w-full justify-start gap-2"
              onClick={() => setDraft({ name: "", ssh_target: "" })}
            >
              <Plus className="size-3.5" />
              Add host
            </Button>
          )}

          {/* "Always auto-install codemux-remote on new hosts" —
              skips the consent modal on subsequent installs. Stored
              in localStorage because it's a per-device decision
              (different machines may have different SSH key
              access). */}
          <AutoInstallToggle />
        </div>
      </div>

      {/* Detail */}
      <div className="flex-1 pl-6">
        {error && (
          <div className="mb-4 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive">
            {error}
          </div>
        )}

        {!selected ? (
          <div className="flex h-full items-center justify-center text-center text-sm text-muted-foreground">
            <div className="space-y-2">
              <Server className="mx-auto size-8 opacity-30" />
              <p>Select a host from the sidebar, or add a new one.</p>
            </div>
          </div>
        ) : editingId === selected.id ? (
          <div className="space-y-4">
            <div className="space-y-1">
              <Label htmlFor="host-edit-name">Name</Label>
              <Input
                id="host-edit-name"
                value={editDraft.name}
                onChange={(e) =>
                  setEditDraft({ ...editDraft, name: e.target.value })
                }
                autoFocus
              />
            </div>
            <div className="space-y-1">
              <Label htmlFor="host-edit-target">SSH target</Label>
              <Input
                id="host-edit-target"
                value={editDraft.ssh_target}
                onChange={(e) =>
                  setEditDraft({ ...editDraft, ssh_target: e.target.value })
                }
              />
            </div>
            <div className="flex justify-end gap-2">
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={handleCancelEdit}
              >
                <X className="mr-1 size-3.5" />
                Cancel
              </Button>
              <Button
                type="button"
                variant="secondary"
                size="sm"
                onClick={handleSaveEdit}
              >
                <Check className="mr-1 size-3.5" />
                Save
              </Button>
            </div>
          </div>
        ) : (
          <div className="space-y-6">
            <div>
              <div className="mb-1 flex items-center gap-2">
                <h2 className="text-base font-semibold">{selected.name}</h2>
                {selected.dirty && (
                  <span className="rounded-full bg-amber-500/15 px-2 py-0.5 text-[10px] font-medium text-amber-500">
                    Pending sync
                  </span>
                )}
              </div>
              <p className="font-mono text-xs text-muted-foreground">
                {selected.ssh_target}
              </p>
            </div>

            <div className="rounded-md border border-border bg-muted/20 p-3">
              <p className="mb-2 text-xs font-medium">Test connection</p>
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={testingId === selected.id}
                onClick={() => void handleTestConnection(selected)}
              >
                {testingId === selected.id ? (
                  <>
                    <Loader2 className="mr-2 size-3.5 animate-spin" />
                    Testing…
                  </>
                ) : (
                  "Test now"
                )}
              </Button>
              {testResults[selected.id] && (
                <div className="mt-2 space-y-2">
                  <p
                    className={cn(
                      "text-xs",
                      testResults[selected.id].ok
                        ? "text-emerald-500"
                        : "text-muted-foreground",
                    )}
                  >
                    {testResults[selected.id].message}
                  </p>
                  {testResults[selected.id].needs_install &&
                    testResults[selected.id].uname && (
                      <Button
                        type="button"
                        variant="secondary"
                        size="sm"
                        disabled={installingId === selected.id}
                        onClick={() =>
                          void handleInstallRemote(
                            selected,
                            testResults[selected.id].uname as string,
                          )
                        }
                      >
                        {installingId === selected.id ? (
                          <>
                            <Loader2 className="mr-2 size-3.5 animate-spin" />
                            Installing…
                          </>
                        ) : (
                          "Install codemux-remote on this host"
                        )}
                      </Button>
                    )}
                </div>
              )}
            </div>

            <div className="flex justify-between border-t border-border pt-4">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => handleStartEdit(selected)}
              >
                <Pencil className="mr-1 size-3.5" />
                Edit
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="text-destructive hover:bg-destructive/10 hover:text-destructive"
                onClick={() => void handleDelete(selected)}
              >
                <Trash2 className="mr-1 size-3.5" />
                Remove
              </Button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function byNameInsensitive(a: HostView, b: HostView): number {
  return a.name.toLowerCase().localeCompare(b.name.toLowerCase());
}

const AUTO_INSTALL_KEY = "codemux.hosts.autoInstallRemote";

function AutoInstallToggle() {
  const [enabled, setEnabled] = useState(false);
  useEffect(() => {
    setEnabled(localStorage.getItem(AUTO_INSTALL_KEY) === "1");
  }, []);
  return (
    <label className="mt-3 flex items-start gap-2 text-[11px] text-muted-foreground cursor-pointer">
      <input
        type="checkbox"
        className="mt-0.5 size-3 shrink-0"
        checked={enabled}
        onChange={(e) => {
          const next = e.target.checked;
          setEnabled(next);
          if (next) {
            localStorage.setItem(AUTO_INSTALL_KEY, "1");
          } else {
            localStorage.removeItem(AUTO_INSTALL_KEY);
          }
        }}
      />
      <span className="leading-tight">
        Always install codemux-remote automatically when missing
      </span>
    </label>
  );
}
