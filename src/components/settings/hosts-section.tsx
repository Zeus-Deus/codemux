import { useCallback, useEffect, useMemo, useState } from "react";

import {
  Cloud,
  Cpu,
  Laptop,
  Loader2,
  Pencil,
  Plus,
  RefreshCw,
  Server,
  Trash2,
  X,
  Check,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import {
  hostsAdd,
  hostsBootstrapInstall,
  hostsDelete,
  hostsList,
  hostsReinstallRemote,
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
/**
 * The "kind" the user picks from the chips in Add Device. Drives only
 * the placeholder hints on the form below — never stored, never sent
 * to the server. The point is to make a first-time user understand
 * that their home machine counts just as much as a paid VPS.
 */
type DeviceKind = "home" | "always-on" | "cloud";

const DEVICE_KINDS: Array<{
  id: DeviceKind;
  label: string;
  icon: typeof Laptop;
  namePlaceholder: string;
  sshPlaceholder: string;
  hint: string;
}> = [
  {
    id: "home",
    label: "Home desktop",
    icon: Laptop,
    namePlaceholder: "home-mac",
    sshPlaceholder: "you@192.168.1.10",
    hint: "Already SSH into it from this device? You're set — paste the same user@host string you use in your terminal.",
  },
  {
    id: "always-on",
    label: "Always-on box",
    icon: Cpu,
    namePlaceholder: "pi",
    sshPlaceholder: "pi@raspberrypi.local",
    hint: "Best for keeping work running after you close your laptop. Pi, mini-PC, NAS, anything reachable over SSH.",
  },
  {
    id: "cloud",
    label: "Cloud server",
    icon: Cloud,
    namePlaceholder: "vps-fra",
    sshPlaceholder: "ubuntu@5.5.5.5",
    hint: "Anything ssh accepts. Your keys + config in ~/.ssh/ are used as-is.",
  },
];

export function HostsSection() {
  const [hosts, setHosts] = useState<HostView[]>([]);
  const [loading, setLoading] = useState(true);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Add-host form draft. `null` means the form isn't open.
  // The "kind" the user picked from the chips. Drives the placeholder
  // hints in the form — purely cosmetic, never stored or pushed.
  // Reset to `null` whenever the draft is closed.
  const [draftKind, setDraftKind] = useState<DeviceKind | null>(null);
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
  const [reinstallingId, setReinstallingId] = useState<number | null>(null);

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
      setError("Device name and SSH target are both required.");
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
      setError("Device name and SSH target are both required.");
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
      `Remove "${host.name}" from your devices? Your SSH config and keys are not affected.`,
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

  // Force a fresh codemux-remote onto the host and restart its daemon.
  // The dev-workflow escape hatch (issue #24): rebuilding the agent
  // keeps the version string the same, so the push-time version check
  // skips the upgrade and the host keeps running the stale binary. This
  // re-uploads the freshly built bits unconditionally, so the next push
  // uses them — no manual scp + pkill needed. Unlike Install, this needs
  // no prior Test connection: the backend re-probes the uname itself.
  const handleReinstallRemote = useCallback(async (host: HostView) => {
    setReinstallingId(host.id);
    try {
      const result = await hostsReinstallRemote(host.id);
      if (result.ok) {
        toast.success("Agent reinstalled", { description: result.message });
      } else {
        toast.error("Reinstall failed", { description: result.message });
      }
    } catch (err) {
      toast.error("Reinstall failed", {
        description: typeof err === "string" ? err : String(err),
      });
    } finally {
      setReinstallingId(null);
    }
  }, []);

  if (loading) {
    return (
      <div className="flex items-center justify-center py-12 text-sm text-muted-foreground">
        <Loader2 className="mr-2 size-4 animate-spin" />
        Loading hosts…
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-[420px] gap-6">
      {/* Sidebar */}
      <div className="w-56 shrink-0 border-r border-border/60 pr-5 flex flex-col">
        <div className="mb-3 flex items-end justify-between gap-2">
          <p className="text-[10px] font-semibold uppercase tracking-[0.14em] text-muted-foreground/60">
            Hosts
          </p>
          <span className="text-[11px] text-muted-foreground/60 tabular-nums">
            {hosts.length}
          </span>
        </div>

        {hosts.length === 0 && !draft && (
          <div className="rounded-lg border border-dashed border-border/60 p-3 text-[12px] text-muted-foreground/80 leading-relaxed">
            No remote hosts yet. Add one to push workspaces from your laptop to a
            server you can SSH into.
          </div>
        )}

        <ul className="space-y-px">
          {hosts.map((host) => {
            const result = testResults[host.id];
            const isOnline = result?.ok === true;
            return (
              <li key={host.id}>
                <button
                  type="button"
                  onClick={() => setSelectedId(host.id)}
                  className={cn(
                    "group/host flex w-full items-center gap-2.5 rounded-md px-2.5 h-8 text-left text-[13px] transition-colors",
                    selectedId === host.id
                      ? "bg-muted text-foreground"
                      : "text-muted-foreground hover:bg-muted/40 hover:text-foreground",
                  )}
                >
                  <span
                    aria-hidden
                    className={cn(
                      "size-1.5 shrink-0 rounded-full transition-colors",
                      isOnline ? "bg-success" : "bg-muted-foreground/40",
                    )}
                  />
                  <span className="min-w-0 flex-1 truncate">{host.name}</span>
                  {host.dirty && (
                    <span
                      title="Pending sync"
                      className="size-1.5 shrink-0 rounded-full bg-warning"
                    />
                  )}
                </button>
              </li>
            );
          })}
        </ul>

        <div className="mt-4 pt-4 border-t border-border/40">
          {draft ? (
            <div className="space-y-3 rounded-lg border border-border/60 bg-muted/30 p-3">
              {/* Device-kind chips. Picking one pre-fills the
                  placeholder hints in the form below — cosmetic
                  only, never stored. Helps a first-time user
                  understand "device" works for their home Mac just
                  as well as a cloud VPS. */}
              <div className="space-y-1.5">
                <Label className="text-[11px] text-muted-foreground/85 font-normal">
                  What kind of device?
                </Label>
                <div className="grid grid-cols-3 gap-1.5">
                  {DEVICE_KINDS.map((kind) => (
                    <button
                      key={kind.id}
                      type="button"
                      onClick={() => setDraftKind(kind.id)}
                      className={cn(
                        "flex flex-col items-center gap-1 rounded-md border px-2 py-2 text-center transition-colors",
                        draftKind === kind.id
                          ? "border-status-remote/40 bg-status-remote/10 text-foreground"
                          : "border-border/60 bg-background/40 text-muted-foreground hover:border-border hover:bg-muted/30 hover:text-foreground",
                      )}
                    >
                      <kind.icon
                        className={cn(
                          "size-4",
                          draftKind === kind.id
                            ? "text-status-remote"
                            : "text-muted-foreground/70",
                        )}
                      />
                      <span className="text-[11px] font-medium leading-tight">
                        {kind.label}
                      </span>
                    </button>
                  ))}
                </div>
              </div>

              <div className="space-y-1.5">
                <Label htmlFor="host-add-name" className="text-[11px] text-muted-foreground/85 font-normal">
                  Name
                </Label>
                <Input
                  id="host-add-name"
                  placeholder={
                    draftKind
                      ? DEVICE_KINDS.find((k) => k.id === draftKind)
                          ?.namePlaceholder ?? "homelab"
                      : "homelab"
                  }
                  value={draft.name}
                  onChange={(e) =>
                    setDraft({ ...draft, name: e.target.value })
                  }
                  autoFocus
                  className="h-8 text-[13px]"
                />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="host-add-target" className="text-[11px] text-muted-foreground/85 font-normal">
                  SSH target
                </Label>
                <Input
                  id="host-add-target"
                  placeholder={
                    draftKind
                      ? DEVICE_KINDS.find((k) => k.id === draftKind)
                          ?.sshPlaceholder ?? "user@host"
                      : "user@host"
                  }
                  value={draft.ssh_target}
                  onChange={(e) =>
                    setDraft({ ...draft, ssh_target: e.target.value })
                  }
                  className="h-8 text-[13px] font-mono"
                />
                <p className="text-[11px] text-muted-foreground/70 leading-relaxed">
                  {draftKind
                    ? DEVICE_KINDS.find((k) => k.id === draftKind)?.hint
                    : "Anything ssh accepts. Your keys + config in ~/.ssh/ are used as-is."}
                </p>
              </div>
              <div className="flex justify-end gap-1.5 pt-1">
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="h-7 px-3 text-[12px]"
                  onClick={() => {
                    setDraft(null);
                    setDraftKind(null);
                    setError(null);
                  }}
                >
                  Cancel
                </Button>
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  className="h-7 px-3 text-[12px]"
                  onClick={handleAdd}
                >
                  Add
                </Button>
              </div>
            </div>
          ) : (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="w-full justify-start gap-2 h-8 px-2.5 text-[13px] text-muted-foreground hover:text-foreground hover:bg-muted/40 border border-dashed border-border/60"
              onClick={() => {
                setDraft({ name: "", ssh_target: "" });
                setDraftKind(null);
              }}
            >
              <Plus className="size-3.5" />
              Add device
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
      <div className="flex-1 min-w-0">
        {error && (
          <div className="mb-4 rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-[12px] text-destructive leading-relaxed">
            {error}
          </div>
        )}

        {!selected ? (
          <div className="flex h-full items-center justify-center text-center">
            <div className="space-y-3">
              <div className="mx-auto size-12 rounded-full bg-muted/40 border border-border/40 flex items-center justify-center">
                <Server className="size-5 text-muted-foreground/60" />
              </div>
              <p className="text-[13px] text-muted-foreground/80">
                Select a host from the list, or add a new one.
              </p>
            </div>
          </div>
        ) : editingId === selected.id ? (
          <div className="space-y-5">
            <div className="space-y-1.5">
              <Label htmlFor="host-edit-name" className="text-[13px] font-medium text-foreground">Name</Label>
              <Input
                id="host-edit-name"
                value={editDraft.name}
                onChange={(e) =>
                  setEditDraft({ ...editDraft, name: e.target.value })
                }
                autoFocus
                className="h-9 text-[13px]"
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="host-edit-target" className="text-[13px] font-medium text-foreground">SSH target</Label>
              <Input
                id="host-edit-target"
                value={editDraft.ssh_target}
                onChange={(e) =>
                  setEditDraft({ ...editDraft, ssh_target: e.target.value })
                }
                className="h-9 text-[13px] font-mono"
              />
            </div>
            <div className="flex justify-end gap-1.5 pt-2">
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="h-8 gap-1.5 text-[12px]"
                onClick={handleCancelEdit}
              >
                <X className="size-3.5" />
                Cancel
              </Button>
              <Button
                type="button"
                variant="secondary"
                size="sm"
                className="h-8 gap-1.5 text-[12px]"
                onClick={handleSaveEdit}
              >
                <Check className="size-3.5" />
                Save
              </Button>
            </div>
          </div>
        ) : (
          <div className="space-y-6">
            <div>
              <div className="mb-1 flex items-center gap-2">
                <h3 className="text-[15px] font-semibold tracking-tight text-foreground">{selected.name}</h3>
                {selected.dirty && (
                  <span className="rounded-full bg-warning/15 border border-warning/30 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wider text-warning">
                    Pending sync
                  </span>
                )}
              </div>
              <p className="font-mono text-[12px] text-muted-foreground/85">
                {selected.ssh_target}
              </p>
            </div>

            <div className="rounded-lg border border-border/60 bg-muted/30 p-4">
              <div className="mb-3 flex items-center justify-between gap-3">
                <div className="min-w-0">
                  <p className="text-[13px] font-medium text-foreground">Test connection</p>
                  <p className="text-[12px] text-muted-foreground/75 leading-relaxed mt-0.5">
                    Probes SSH reachability and the remote codemux-remote helper.
                  </p>
                </div>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="h-8 text-[12px] gap-1.5 shrink-0"
                  disabled={testingId === selected.id}
                  onClick={() => void handleTestConnection(selected)}
                >
                  {testingId === selected.id ? (
                    <>
                      <Loader2 className="size-3.5 animate-spin" />
                      Testing…
                    </>
                  ) : (
                    "Test now"
                  )}
                </Button>
              </div>
              {testResults[selected.id] && (
                <div className="space-y-2.5 pt-3 border-t border-border/40">
                  <div className="flex items-start gap-2">
                    <span
                      className={cn(
                        "size-1.5 rounded-full shrink-0 mt-1.5",
                        testResults[selected.id].ok ? "bg-success" : "bg-muted-foreground/50",
                      )}
                    />
                    <p
                      className={cn(
                        "text-[12px] leading-relaxed",
                        testResults[selected.id].ok
                          ? "text-success"
                          : "text-muted-foreground/85",
                      )}
                    >
                      {testResults[selected.id].message}
                    </p>
                  </div>
                  {testResults[selected.id].needs_install &&
                    testResults[selected.id].uname && (
                      <Button
                        type="button"
                        variant="secondary"
                        size="sm"
                        className="h-8 text-[12px]"
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
                            <Loader2 className="mr-1.5 size-3.5 animate-spin" />
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

            {/* Reinstall agent — dev-workflow escape hatch (issue #24).
                The push-time version check skips the upgrade when the
                version string is unchanged (which it always is across
                local rebuilds), so this re-uploads the freshly built
                codemux-remote and restarts its daemon unconditionally. */}
            <div className="rounded-lg border border-border/60 bg-muted/30 p-4">
              <div className="flex items-center justify-between gap-3">
                <div className="min-w-0">
                  <p className="text-[13px] font-medium text-foreground">Reinstall agent</p>
                  <p className="text-[12px] text-muted-foreground/75 leading-relaxed mt-0.5">
                    Re-upload codemux-remote and restart it on the host. Use after
                    rebuilding the agent locally — pushes skip the update when the
                    version string is unchanged.
                  </p>
                </div>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="h-8 text-[12px] gap-1.5 shrink-0"
                  disabled={reinstallingId === selected.id}
                  onClick={() => void handleReinstallRemote(selected)}
                >
                  {reinstallingId === selected.id ? (
                    <>
                      <Loader2 className="size-3.5 animate-spin" />
                      Reinstalling…
                    </>
                  ) : (
                    <>
                      <RefreshCw className="size-3.5" />
                      Reinstall
                    </>
                  )}
                </Button>
              </div>
            </div>

            <div className="flex items-center justify-between pt-4 border-t border-border/40">
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-8 gap-1.5 text-[12px]"
                onClick={() => handleStartEdit(selected)}
              >
                <Pencil className="size-3.5" />
                Edit
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="h-8 gap-1.5 text-[12px] text-destructive hover:bg-destructive/10 hover:text-destructive"
                onClick={() => void handleDelete(selected)}
              >
                <Trash2 className="size-3.5" />
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
    <label className="mt-3 flex items-start gap-2 text-[12px] text-muted-foreground/85 cursor-pointer leading-relaxed select-none hover:text-foreground transition-colors">
      <input
        type="checkbox"
        className="mt-0.5 size-3 shrink-0 accent-foreground"
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
      <span>
        Always install codemux-remote automatically when missing
      </span>
    </label>
  );
}
