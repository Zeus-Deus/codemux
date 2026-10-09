import { useCallback, useEffect, useRef, useState } from "react";

import {
  Check,
  Loader2,
  Minus,
  Monitor,
  MoreHorizontal,
  Plus,
  Server,
  X,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  describeStatus,
  type DeviceTone,
} from "@/components/devices/use-device-cards";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import {
  hostsAdd,
  hostsBootstrapInstall,
  hostsDelete,
  hostsReinstallRemote,
  hostsSshConfigHosts,
  hostsTestConnection,
  hostsUpdate,
  type HostStatusView,
  type HostTestResult,
  type HostView,
} from "@/tauri/commands";
import { useHosts, useHostsStore } from "@/stores/hosts-store";
import { useHostStatusStore, useHostStatuses } from "@/stores/host-status-store";
import { useLocalDeviceName } from "@/stores/local-device-store";
import { useAddDeviceDialogStore } from "@/stores/add-device-dialog-store";
import { SettingsCard, SubsectionHeader } from "./settings-primitives";

/**
 * Settings → Devices: this machine, plus every other machine Codemux can run
 * threads on over SSH. The list and reachability come from the shared hosts
 * and host-status stores, so this page, the composer's device picker and the
 * sidebar always agree.
 *
 * SSH credentials are never part of any payload. Auth happens at the OS
 * level via the user's `~/.ssh/config`, agent, and known_hosts.
 */

/** A connection test run from this page, and the poller row it was taken
 *  against. */
interface DeviceTest {
  result: HostTestResult;
  observed: HostStatusView | undefined;
}

type RecordTest = (hostId: number, result: HostTestResult) => void;

const STATUS_LABEL: Record<DeviceTone, string> = {
  online: "Connected",
  updating: "Updating…",
  attention: "Needs setup",
  offline: "Offline",
  checking: "Checking…",
};

const STATUS_DOT: Record<DeviceTone, string> = {
  online: "bg-status-open",
  updating: "bg-status-remote motion-safe:animate-pulse",
  attention: "bg-status-working",
  offline: "bg-muted-foreground/60",
  checking: "bg-muted-foreground/40",
};

function isReady(result: HostTestResult): boolean {
  return result.ok && !result.needs_install;
}

/**
 * How a row reads. The background poller only visits devices that have
 * synced to the account, and a test run here is not folded into its store,
 * so the latest test stands in until the poller reports something newer.
 */
function deviceStatus(
  status: HostStatusView | undefined,
  test: DeviceTest | undefined,
): { tone: DeviceTone; detail: string | null } {
  if (test && (!status?.probed || status === test.observed)) {
    if (isReady(test.result)) return { tone: "online", detail: null };
    return {
      tone: test.result.needs_install ? "attention" : "offline",
      detail: test.result.message,
    };
  }
  const summary = describeStatus(status ?? null, Date.now());
  return { tone: summary.tone, detail: summary.detail };
}

/** Re-read both device stores after a change, so every surface sees it. */
function refreshDevices(): Promise<unknown> {
  return Promise.all([
    useHostsStore.getState().refresh(),
    useHostStatusStore.getState().refresh(),
  ]);
}

function errorMessage(err: unknown): string {
  if (typeof err === "string") return err;
  return err instanceof Error ? err.message : String(err);
}

/** The name a device gets when the user leaves Name empty:
 *  `deus@zeus.local` → `zeus`. IP addresses stay whole. */
export function defaultDeviceName(sshTarget: string): string {
  const trimmed = sshTarget.trim();
  const host = trimmed.slice(trimmed.lastIndexOf("@") + 1);
  if (/^[\d.]+$/.test(host) || host.includes(":")) return host;
  return host.split(".")[0] || host;
}

export function HostsSection() {
  const hosts = useHosts();
  const loaded = useHostsStore((s) => s.loaded);
  const loadError = useHostsStore((s) => s.error);
  const statuses = useHostStatuses();
  const localName = useLocalDeviceName();
  const setAddOpen = useAddDeviceDialogStore((s) => s.setOpen);

  const [tests, setTests] = useState<Record<number, DeviceTest>>({});
  const [busy, setBusy] = useState<Record<number, "testing" | "setup">>({});
  const [renaming, setRenaming] = useState<HostView | null>(null);
  const [removing, setRemoving] = useState<HostView | null>(null);

  const recordTest = useCallback<RecordTest>((hostId, result) => {
    const observed = useHostStatusStore.getState().statuses[hostId];
    setTests((prev) => ({ ...prev, [hostId]: { result, observed } }));
  }, []);

  const setHostBusy = (hostId: number, state: "testing" | "setup" | null) =>
    setBusy((prev) => {
      const next = { ...prev };
      if (state) next[hostId] = state;
      else delete next[hostId];
      return next;
    });

  const testConnection = async (host: HostView) => {
    setHostBusy(host.id, "testing");
    try {
      const result = await hostsTestConnection(host.id);
      recordTest(host.id, result);
      if (isReady(result)) {
        toast.success(`${host.name} is connected`);
      } else if (result.needs_install) {
        toast.warning(`${host.name} needs setup`, {
          description: "Choose Set up again to install the Codemux helper.",
        });
      } else {
        toast.error(`Couldn't reach ${host.name}`, { description: result.message });
      }
    } catch (err) {
      recordTest(host.id, { ok: false, message: errorMessage(err) });
      toast.error(`Couldn't reach ${host.name}`, { description: errorMessage(err) });
    } finally {
      setHostBusy(host.id, null);
    }
  };

  // Probe first. A missing, broken or out-of-date helper is installed over
  // in place (the probe's uname picks the binary), which leaves the
  // device's running terminals alone. The forced reinstall restarts its
  // terminal daemon, so it's only the fallback: no uname to go on, or a
  // repair of a device that already reports ready.
  const setUpAgain = async (host: HostView) => {
    setHostBusy(host.id, "setup");
    try {
      const probe = await hostsTestConnection(host.id);
      recordTest(host.id, probe);
      if (!probe.ok && !probe.needs_install) {
        toast.error(`Couldn't reach ${host.name}`, { description: probe.message });
        return;
      }
      const install =
        probe.needs_install && probe.uname
          ? await hostsBootstrapInstall(host.id, probe.uname)
          : await hostsReinstallRemote(host.id);
      if (!install.ok) {
        toast.error(`Couldn't set up ${host.name}`, { description: install.message });
        return;
      }
      await refreshDevices();
      const check = await hostsTestConnection(host.id);
      recordTest(host.id, check);
      if (isReady(check)) {
        toast.success(`${host.name} is ready`);
      } else {
        toast.error(`Couldn't set up ${host.name}`, { description: check.message });
      }
    } catch (err) {
      toast.error(`Couldn't set up ${host.name}`, { description: errorMessage(err) });
    } finally {
      setHostBusy(host.id, null);
    }
  };

  const rename = async (host: HostView, name: string) => {
    await hostsUpdate(host.id, name, host.ssh_target);
    await refreshDevices();
  };

  const remove = async (host: HostView) => {
    try {
      await hostsDelete(host.id);
      setTests((prev) => {
        const next = { ...prev };
        delete next[host.id];
        return next;
      });
      await refreshDevices();
    } catch (err) {
      toast.error(`Couldn't remove ${host.name}`, { description: errorMessage(err) });
    }
  };

  return (
    <div>
      <SubsectionHeader title="This machine" />
      <SettingsCard className="flex items-center gap-3 px-4 py-3">
        <Monitor className="size-4 shrink-0 text-muted-foreground" aria-hidden />
        <div className="min-w-0">
          <p className="truncate text-body font-medium text-foreground">
            {localName ?? "This machine"}
          </p>
          <p className="text-label text-muted-foreground/80">
            This computer · always available
          </p>
        </div>
      </SettingsCard>

      <SubsectionHeader
        className="mt-8"
        title="Devices"
        action={
          <Button
            type="button"
            variant="ghost"
            size="xs"
            className="text-muted-foreground"
            onClick={() => setAddOpen(true)}
          >
            <Plus />
            Add device
          </Button>
        }
      />
      <SettingsCard className="p-0">
        {!loaded ? (
          <p className="flex items-center gap-2 px-4 py-3 text-body-sm text-muted-foreground">
            <Loader2 className="size-3.5 animate-spin" aria-hidden />
            Loading devices…
          </p>
        ) : hosts.length === 0 ? (
          <p className="px-4 py-4 text-body-sm leading-relaxed text-muted-foreground/80">
            {loadError
              ? `Couldn't load devices: ${loadError}`
              : "No devices yet. Add a home server, a desktop, or a cloud VM — anything you can SSH into."}
          </p>
        ) : (
          <ul className="divide-y divide-border/40">
            {hosts.map((host) => (
              <DeviceRow
                key={host.id}
                host={host}
                status={deviceStatus(statuses[host.id], tests[host.id])}
                busy={busy[host.id] ?? null}
                onTest={() => void testConnection(host)}
                onSetUp={() => void setUpAgain(host)}
                onRename={() => setRenaming(host)}
                onRemove={() => setRemoving(host)}
              />
            ))}
          </ul>
        )}
      </SettingsCard>

      <AddDeviceDialog hosts={hosts} onTested={recordTest} />

      <Dialog
        open={renaming !== null}
        onOpenChange={(open) => {
          if (!open) setRenaming(null);
        }}
      >
        <DialogContent className="sm:max-w-sm">
          {renaming && (
            <RenameDeviceForm
              host={renaming}
              onRename={rename}
              onDone={() => setRenaming(null)}
            />
          )}
        </DialogContent>
      </Dialog>

      <AlertDialog
        open={removing !== null}
        onOpenChange={(open) => {
          if (!open) setRemoving(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove {removing?.name}?</AlertDialogTitle>
            <AlertDialogDescription>
              Codemux forgets this device. Nothing on it is deleted, and your
              SSH config and keys stay as they are.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                if (removing) void remove(removing);
              }}
            >
              Remove
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function DeviceRow({
  host,
  status,
  busy,
  onTest,
  onSetUp,
  onRename,
  onRemove,
}: {
  host: HostView;
  status: { tone: DeviceTone; detail: string | null };
  busy: "testing" | "setup" | null;
  onTest: () => void;
  onSetUp: () => void;
  onRename: () => void;
  onRemove: () => void;
}) {
  return (
    <li className="flex items-center gap-3 px-4 py-2.5">
      <Server className="size-4 shrink-0 text-muted-foreground" aria-hidden />
      <div className="min-w-0 flex-1">
        <p className="truncate text-body font-medium text-foreground">{host.name}</p>
        <p
          className="flex min-w-0 items-center gap-1.5 text-label text-muted-foreground/80"
          title={status.detail ?? undefined}
        >
          <span className="truncate">SSH {host.ssh_target}</span>
          <span aria-hidden>·</span>
          {busy ? (
            <Loader2 className="size-3 shrink-0 animate-spin" aria-hidden />
          ) : (
            <span
              aria-hidden
              className={cn("size-1.5 shrink-0 rounded-full", STATUS_DOT[status.tone])}
            />
          )}
          <span className="shrink-0">
            {busy === "testing"
              ? "Testing…"
              : busy === "setup"
                ? "Setting up…"
                : STATUS_LABEL[status.tone]}
          </span>
        </p>
      </div>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            className="text-muted-foreground"
            aria-label={`More actions for ${host.name}`}
          >
            <MoreHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-auto min-w-40">
          <DropdownMenuItem disabled={busy !== null} onSelect={onTest}>
            Test connection
          </DropdownMenuItem>
          <DropdownMenuItem disabled={busy !== null} onSelect={onSetUp}>
            Set up again
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={onRename}>Rename…</DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem variant="destructive" onSelect={onRemove}>
            Remove…
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </li>
  );
}

// ── Add device ──────────────────────────────────────────────────

type StepId = "save" | "ssh" | "install" | "ready";
type StepState = "pending" | "running" | "done" | "skipped" | "failed";

const STEPS: { id: StepId; label: string }[] = [
  { id: "save", label: "Save device" },
  { id: "ssh", label: "Connect over SSH" },
  { id: "install", label: "Install Codemux helper" },
  { id: "ready", label: "Ready" },
];

const PENDING_STEPS: Record<StepId, StepState> = {
  save: "pending",
  ssh: "pending",
  install: "pending",
  ready: "pending",
};

/** Bound to the shared store so the composer's device picker can open it
 *  straight from "Add device…". */
function AddDeviceDialog({
  hosts,
  onTested,
}: {
  hosts: readonly HostView[];
  onTested: RecordTest;
}) {
  const open = useAddDeviceDialogStore((s) => s.open);
  const setOpen = useAddDeviceDialogStore((s) => s.setOpen);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="sm:max-w-md">
        <AddDeviceForm hosts={hosts} onTested={onTested} onDone={() => setOpen(false)} />
      </DialogContent>
    </Dialog>
  );
}

/**
 * Lives inside the dialog content, so it unmounts on close and every open
 * starts fresh. Clicking Connect is the consent to install the helper.
 */
function AddDeviceForm({
  hosts,
  onTested,
  onDone,
}: {
  hosts: readonly HostView[];
  onTested: RecordTest;
  onDone: () => void;
}) {
  const [target, setTarget] = useState("");
  const [name, setName] = useState("");
  const [suggestions, setSuggestions] = useState<string[]>([]);
  const [steps, setSteps] = useState<Record<StepId, StepState> | null>(null);
  const [failure, setFailure] = useState<{ message: string; sshHint: boolean } | null>(
    null,
  );
  const [running, setRunning] = useState(false);
  // Kept after a failed attempt so Retry reuses the device instead of
  // adding it twice.
  const [saved, setSaved] = useState<HostView | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  // The flow spans several awaits; once this form unmounts (the dialog
  // closed) it must not start another step or close a newly opened dialog.
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    hostsSshConfigHosts()
      .then((list) => {
        if (!cancelled) setSuggestions(list);
      })
      .catch(() => {
        // Suggestions are a convenience; typing a target still works.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const sshTarget = target.trim();
  const query = sshTarget.toLowerCase();
  const taken = new Set(hosts.map((h) => h.ssh_target.toLowerCase()));
  const matches = suggestions
    .filter((s) => {
      const lower = s.toLowerCase();
      return !taken.has(lower) && lower !== query && lower.includes(query);
    })
    .slice(0, 8);

  const connect = async () => {
    if (!sshTarget || running) return;
    const deviceName = name.trim() || defaultDeviceName(sshTarget);
    const progress = { ...PENDING_STEPS };
    let step: StepId = "save";
    const mark = (id: StepId, state: StepState) => {
      progress[id] = state;
      if (alive.current) setSteps({ ...progress });
    };
    const fail = (message: string) => {
      mark(step, "failed");
      if (alive.current) setFailure({ message, sshHint: step === "ssh" });
    };

    setRunning(true);
    setFailure(null);
    try {
      mark("save", "running");
      // A device already saved for this target (say, from an attempt the
      // dialog was closed on) continues setup instead of being added twice.
      // Same case-insensitive match the suggestion chips use.
      const existing = hosts.find((h) => h.ssh_target.toLowerCase() === query);
      let host: HostView;
      if (existing) {
        // Its name only changes when one is typed.
        const wanted = name.trim() || existing.name;
        host =
          wanted === existing.name
            ? existing
            : await hostsUpdate(existing.id, wanted, existing.ssh_target);
      } else if (!saved) {
        host = await hostsAdd(deviceName, sshTarget);
      } else if (saved.ssh_target !== sshTarget || saved.name !== deviceName) {
        // A retry after fixing a typo updates the device it already saved.
        host = await hostsUpdate(saved.id, deviceName, sshTarget);
      } else {
        host = saved;
      }
      // Only a device this dialog added is rewritten on a later retry.
      if (alive.current && !existing) setSaved(host);
      void refreshDevices();
      mark("save", "done");
      if (!alive.current) return;

      step = "ssh";
      mark("ssh", "running");
      const probe = await hostsTestConnection(host.id);
      onTested(host.id, probe);
      if (!probe.ok && !probe.needs_install) return fail(probe.message);
      mark("ssh", "done");
      if (!alive.current) return;

      step = "install";
      if (probe.needs_install) {
        mark("install", "running");
        const install = probe.uname
          ? await hostsBootstrapInstall(host.id, probe.uname)
          : await hostsReinstallRemote(host.id);
        if (!install.ok) return fail(install.message);
        mark("install", "done");
      } else {
        mark("install", "skipped");
      }
      if (!alive.current) return;

      step = "ready";
      mark("ready", "running");
      await refreshDevices();
      const check = await hostsTestConnection(host.id);
      onTested(host.id, check);
      if (!isReady(check)) return fail(check.message);
      mark("ready", "done");
      toast.success(`${host.name} is ready`);
      // `onDone` closes whichever dialog is open now, which may be a new
      // one opened after this one was closed mid-run.
      if (alive.current) onDone();
    } catch (err) {
      fail(errorMessage(err));
    } finally {
      if (alive.current) setRunning(false);
    }
  };

  return (
    <form
      className="grid gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        void connect();
      }}
    >
      <DialogHeader>
        <DialogTitle>Add device</DialogTitle>
        <DialogDescription className="text-body-sm leading-relaxed">
          Run threads on another machine you can reach over SSH. Your keys and
          ~/.ssh/config are used as-is.
        </DialogDescription>
      </DialogHeader>

      <div className="grid gap-1.5">
        <Label htmlFor="add-device-target" className="text-body-sm">
          SSH host
        </Label>
        <Input
          id="add-device-target"
          ref={inputRef}
          value={target}
          onChange={(e) => setTarget(e.target.value)}
          placeholder="user@host or an ~/.ssh/config alias"
          autoComplete="off"
          spellCheck={false}
          autoFocus
          disabled={running}
          className="font-mono"
        />
        {!running && matches.length > 0 && (
          <div className="flex flex-wrap items-center gap-1.5 pt-0.5">
            <span className="text-label text-muted-foreground/70">From your SSH config</span>
            {matches.map((suggestion) => (
              <button
                key={suggestion}
                type="button"
                onClick={() => {
                  setTarget(suggestion);
                  inputRef.current?.focus();
                }}
                className="rounded-sm border border-border/60 bg-surface-1 px-1.5 py-0.5 font-mono text-label text-muted-foreground transition-colors duration-100 hover:bg-surface-2 hover:text-foreground"
              >
                {suggestion}
              </button>
            ))}
          </div>
        )}
      </div>

      <div className="grid gap-1.5">
        <Label htmlFor="add-device-name" className="text-body-sm">
          Name <span className="font-normal text-muted-foreground/70">optional</span>
        </Label>
        <Input
          id="add-device-name"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder={sshTarget ? defaultDeviceName(sshTarget) : "homelab"}
          autoComplete="off"
          disabled={running}
        />
      </div>

      {steps ? (
        <ol className="grid gap-1.5" aria-live="polite">
          {STEPS.map(({ id, label }) => (
            <li key={id} data-state={steps[id]} className="flex items-start gap-2">
              <span className="mt-0.5 flex size-4 shrink-0 items-center justify-center">
                <StepIcon state={steps[id]} />
              </span>
              <div className="min-w-0">
                <p
                  className={cn(
                    "text-body-sm",
                    steps[id] === "pending" ? "text-muted-foreground/70" : "text-foreground",
                  )}
                >
                  {label}
                  {steps[id] === "skipped" && (
                    <span className="text-muted-foreground/70"> · already installed</span>
                  )}
                </p>
                {steps[id] === "failed" && failure && (
                  <div className="mt-0.5 space-y-0.5 text-label leading-relaxed">
                    <p className="select-text text-destructive">{failure.message}</p>
                    {failure.sshHint && (
                      <p className="text-muted-foreground/80">
                        Make sure{" "}
                        <code className="font-mono text-foreground">ssh {sshTarget}</code>{" "}
                        works from a terminal without a password prompt.
                      </p>
                    )}
                  </div>
                )}
              </div>
            </li>
          ))}
        </ol>
      ) : (
        <p className="text-label leading-relaxed text-muted-foreground/80">
          If the small Codemux helper is missing, Connect installs it in your
          user account. No root needed.
        </p>
      )}

      <DialogFooter>
        <Button type="submit" disabled={!sshTarget || running}>
          {running && <Loader2 className="animate-spin" aria-hidden />}
          {running ? "Connecting…" : failure ? "Retry" : "Connect"}
        </Button>
      </DialogFooter>
    </form>
  );
}

function StepIcon({ state }: { state: StepState }) {
  switch (state) {
    case "running":
      return <Loader2 className="size-3.5 animate-spin text-muted-foreground" aria-hidden />;
    case "done":
      return <Check className="size-3.5 text-status-open" aria-hidden />;
    case "skipped":
      return <Minus className="size-3.5 text-muted-foreground/60" aria-hidden />;
    case "failed":
      return <X className="size-3.5 text-destructive" aria-hidden />;
    default:
      return <span aria-hidden className="size-1.5 rounded-full bg-muted-foreground/40" />;
  }
}

function RenameDeviceForm({
  host,
  onRename,
  onDone,
}: {
  host: HostView;
  onRename: (host: HostView, name: string) => Promise<void>;
  onDone: () => void;
}) {
  const [name, setName] = useState(host.name);
  const [saving, setSaving] = useState(false);
  const trimmed = name.trim();

  const submit = async () => {
    if (!trimmed || trimmed === host.name) return onDone();
    setSaving(true);
    try {
      await onRename(host, trimmed);
      onDone();
    } catch (err) {
      toast.error(`Couldn't rename ${host.name}`, { description: errorMessage(err) });
      setSaving(false);
    }
  };

  return (
    <form
      className="grid gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <DialogHeader>
        <DialogTitle>Rename device</DialogTitle>
        <DialogDescription className="text-body-sm">
          <span className="font-mono">{host.ssh_target}</span>
        </DialogDescription>
      </DialogHeader>
      <Input
        aria-label="Device name"
        value={name}
        onChange={(e) => setName(e.target.value)}
        autoFocus
        disabled={saving}
      />
      <DialogFooter>
        <Button type="submit" disabled={!trimmed || saving}>
          Save
        </Button>
      </DialogFooter>
    </form>
  );
}
