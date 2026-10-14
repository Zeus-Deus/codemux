import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { ProviderUpdateReport } from "@/stores/provider-update-store";
import { updateTargetKey, useProviderUpdates } from "@/stores/provider-update-store";
import type { AgentChatProviderKind } from "@/tauri/types";

/**
 * Settings → Updates: Codemux and every coding agent, on this computer and
 * on each device.
 *
 * Detection and the update itself always run on the machine that owns the
 * CLI (`codemux-remote providers` on a device), so an Omarchy device updates
 * through mise and a plain Linux box through npm or the provider's own
 * updater. The renderer only says which provider; it never sends a command.
 */

export const LOCAL = "local";
export type MachineKey = typeof LOCAL | `host:${number}`;
export const hostKey = (id: number): MachineKey => `host:${id}`;

/** What a device reported. Mirrors `DeviceProviders` in provider_updates.rs. */
export interface DeviceProvidersReport {
  host_id: number;
  helper_version: string | null;
  app_version: string;
  providers: ProviderUpdateReport[] | null;
  error: string | null;
}

interface DeviceHelperUpdate {
  host_id: number;
  status: "current" | "upgraded" | "skipped";
  from: string | null;
  to: string | null;
  message: string | null;
}

export interface MachineState {
  loading: boolean;
  checkedAt?: number;
  providers: ProviderUpdateReport[];
  /** Devices only: the helper's version and this desktop's. */
  helperVersion?: string | null;
  appVersion?: string;
  error?: string | null;
}

export type ItemPhase = "queued" | "updating" | "updated" | "failed";
export interface ItemState {
  phase: ItemPhase;
  message?: string;
}

/** `helper` is a device's Codemux; providers use their kind. */
export type ItemId = AgentChatProviderKind | "helper";
export const itemKey = (machine: MachineKey, item: ItemId) => `${machine}/${item}`;

export interface PlannedItem {
  machine: MachineKey;
  item: ItemId;
  label: string;
}
export interface SkippedItem {
  machine: MachineKey;
  label: string;
  reason: string;
}
export interface UpdatePlan {
  items: PlannedItem[];
  skipped: SkippedItem[];
}

export interface RunSummary {
  updated: number;
  failed: { machine: MachineKey; label: string; reason: string }[];
  at: number;
}

export const PROVIDER_LABELS: Record<AgentChatProviderKind, string> = {
  claude: "Claude",
  codex: "Codex",
  cursor: "Cursor",
  grok: "Grok",
  hermes: "Hermes",
  acp: "Custom agent",
  opencode: "OpenCode",
};

/** `a` is older than `b`. Unparseable versions only compare equal to themselves. */
export function isOlderVersion(a: string, b: string): boolean {
  const parse = (v: string) => v.trim().replace(/^v/, "").split(/[-+]/)[0].split(".").map(Number);
  const [x, y] = [parse(a), parse(b)];
  if (x.some(Number.isNaN) || y.some(Number.isNaN)) return a.trim() !== b.trim();
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    const d = (x[i] ?? 0) - (y[i] ?? 0);
    if (d !== 0) return d < 0;
  }
  return false;
}

export function helperIsOutdated(state: MachineState | undefined): boolean {
  return !!state?.helperVersion && !!state.appVersion && isOlderVersion(state.helperVersion, state.appVersion);
}

/**
 * What "Update all" would do with what is known now. Pure, so the confirm
 * dialog and the run agree and the decision is testable.
 */
export function buildPlan(machines: Record<string, MachineState>, names: Record<string, string>): UpdatePlan {
  const items: PlannedItem[] = [];
  const skipped: SkippedItem[] = [];
  for (const [key, state] of Object.entries(machines) as [MachineKey, MachineState][]) {
    const name = names[key] ?? key;
    if (key !== LOCAL && state.error && !state.helperVersion) {
      skipped.push({ machine: key, label: name, reason: state.error });
      continue;
    }
    if (key !== LOCAL && helperIsOutdated(state)) {
      items.push({ machine: key, item: "helper", label: "Codemux" });
    }
    for (const report of state.providers) {
      if (!report.available) continue;
      const label = PROVIDER_LABELS[report.provider];
      if (report.can_update) items.push({ machine: key, item: report.provider, label });
      else skipped.push({ machine: key, label, reason: report.message ?? "Needs a manual update" });
    }
  }
  return { items, skipped };
}

interface Store {
  machines: Partial<Record<MachineKey, MachineState>>;
  items: Record<string, ItemState>;
  running: boolean;
  summary: RunSummary | null;
  refreshLocal: (force?: boolean) => Promise<void>;
  refreshDevice: (hostId: number, force?: boolean) => Promise<void>;
  refreshAll: (hostIds: number[], force?: boolean) => Promise<void>;
  updateOne: (machine: MachineKey, item: ItemId) => Promise<boolean>;
  /** Runs the plan: helpers and providers per machine (machines in
   *  parallel, one installer at a time on each). The Codemux app itself is
   *  left to the caller, last, because installing it restarts the app. */
  runPlan: (plan: UpdatePlan) => Promise<RunSummary>;
}

export const useUpdateAll = create<Store>((set, get) => {
  const patchMachine = (key: MachineKey, value: Partial<MachineState>) =>
    set((s) => ({
      machines: { ...s.machines, [key]: { providers: [], loading: false, ...s.machines[key], ...value } },
    }));
  const patchItem = (key: string, value: ItemState | undefined) =>
    set((s) => {
      const items = { ...s.items };
      if (value) items[key] = value;
      else delete items[key];
      return { items };
    });

  const replaceReport = (machine: MachineKey, report: ProviderUpdateReport) => {
    const state = get().machines[machine];
    if (!state) return;
    patchMachine(machine, {
      providers: state.providers.map((p) => (p.provider === report.provider ? report : p)),
    });
    // The chat's per-provider notice reads the same answer.
    if (machine === LOCAL) {
      useProviderUpdates.setState((s) => {
        const key = updateTargetKey({ provider: report.provider });
        return { slots: { ...s.slots, [key]: { ...s.slots[key], report, updated: true, checkedAt: Date.now() } } };
      });
    }
  };

  return {
    machines: {},
    items: {},
    running: false,
    summary: null,

    async refreshLocal(force = false) {
      if (get().machines[LOCAL]?.loading) return;
      patchMachine(LOCAL, { loading: true, error: null });
      try {
        const providers = await invoke<ProviderUpdateReport[]>("provider_updates_local", { force });
        patchMachine(LOCAL, { providers, checkedAt: Date.now() });
      } catch (error) {
        patchMachine(LOCAL, { error: String(error) });
      } finally {
        patchMachine(LOCAL, { loading: false });
      }
    },

    async refreshDevice(hostId, force = false) {
      const key = hostKey(hostId);
      if (get().machines[key]?.loading) return;
      patchMachine(key, { loading: true });
      try {
        const report = await invoke<DeviceProvidersReport>("provider_updates_device", { hostId, force });
        patchMachine(key, {
          providers: report.providers ?? [],
          helperVersion: report.helper_version,
          appVersion: report.app_version,
          error: report.error,
          checkedAt: Date.now(),
        });
      } catch (error) {
        patchMachine(key, { error: String(error), helperVersion: null });
      } finally {
        patchMachine(key, { loading: false });
      }
    },

    async refreshAll(hostIds, force = false) {
      await Promise.all([get().refreshLocal(force), ...hostIds.map((id) => get().refreshDevice(id, force))]);
    },

    async updateOne(machine, item) {
      const key = itemKey(machine, item);
      if (get().items[key]?.phase === "updating") return false;
      patchItem(key, { phase: "updating" });
      try {
        if (item === "helper") {
          const hostId = Number(machine.slice(5));
          const result = await invoke<DeviceHelperUpdate>("device_helper_update", { hostId });
          if (result.status === "skipped") throw new Error(result.message ?? "The device couldn't be updated");
          patchItem(key, { phase: "updated", message: result.message ?? undefined });
          patchMachine(machine, { helperVersion: result.to ?? get().machines[machine]?.appVersion });
        } else if (machine === LOCAL) {
          replaceReport(machine, await invoke<ProviderUpdateReport>("agent_chat_provider_update", { provider: item }));
          patchItem(key, { phase: "updated" });
        } else {
          const hostId = Number(machine.slice(5));
          replaceReport(machine, await invoke<ProviderUpdateReport>("provider_update_device", { hostId, provider: item }));
          patchItem(key, { phase: "updated" });
        }
        return true;
      } catch (error) {
        patchItem(key, { phase: "failed", message: error instanceof Error ? error.message : String(error) });
        return false;
      }
    },

    async runPlan(plan) {
      set({ running: true, summary: null });
      for (const { machine, item } of plan.items) patchItem(itemKey(machine, item), { phase: "queued" });
      const byMachine = new Map<MachineKey, PlannedItem[]>();
      for (const planned of plan.items) {
        byMachine.set(planned.machine, [...(byMachine.get(planned.machine) ?? []), planned]);
      }
      const failed: RunSummary["failed"] = [];
      let updated = 0;
      try {
        await Promise.all(
          [...byMachine.entries()].map(async ([machine, list]) => {
            // The helper goes first: its `providers update` is what runs the rest.
            const ordered = [...list].sort((a, b) => Number(b.item === "helper") - Number(a.item === "helper"));
            if (ordered[0]?.item === "helper" && (await get().updateOne(machine, "helper"))) {
              updated++;
              ordered.shift();
              // An older helper couldn't list the device's agents. Now it can,
              // so the same run picks up whatever it finds out of date.
              await get().refreshDevice(Number(machine.slice(5)), true);
              const planned = new Set(ordered.map((p) => p.item));
              for (const report of get().machines[machine]?.providers ?? []) {
                if (report.available && report.can_update && !planned.has(report.provider)) {
                  patchItem(itemKey(machine, report.provider), { phase: "queued" });
                  ordered.push({ machine, item: report.provider, label: PROVIDER_LABELS[report.provider] });
                }
              }
            } else if (ordered[0]?.item === "helper") {
              const reason = get().items[itemKey(machine, "helper")]?.message ?? "Update failed";
              failed.push({ machine, label: "Codemux", reason });
              // Without the new helper the device can't run provider updates.
              for (const rest of ordered.slice(1)) patchItem(itemKey(machine, rest.item), undefined);
              return;
            }
            for (const planned of ordered) {
              if (await get().updateOne(machine, planned.item)) updated++;
              else {
                failed.push({
                  machine,
                  label: planned.label,
                  reason: get().items[itemKey(machine, planned.item)]?.message ?? "Update failed",
                });
              }
            }
          }),
        );
      } finally {
        const summary = { updated, failed, at: Date.now() };
        set({ running: false, summary });
      }
      return get().summary!;
    },
  };
});
