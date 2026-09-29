import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { AgentChatProviderKind } from "@/tauri/types";

export interface ProviderUpdateReport {
  provider: AgentChatProviderKind;
  installed_version: string | null;
  latest_version: string | null;
  available: boolean;
  manager: string;
  can_update: boolean;
  message: string | null;
}
export interface UpdateTarget { provider: AgentChatProviderKind; installation?: string }
interface Slot {
  report?: ProviderUpdateReport;
  checkedAt?: number;
  checking?: boolean;
  updating?: boolean;
  updated?: boolean;
  error?: string;
  dismissed?: string;
}
export const updateTargetKey = (target: UpdateTarget) => JSON.stringify([target.provider, target.installation ?? null]);
export const updateIdentity = (report?: ProviderUpdateReport) => report ? `${report.installed_version}:${report.latest_version}` : "";
export const UPDATE_INTERVAL = 60 * 60 * 1000;
interface Store {
  slots: Record<string, Slot>;
  check: (target: UpdateTarget) => Promise<void>;
  update: (target: UpdateTarget) => Promise<void>;
  dismiss: (target: UpdateTarget) => void;
}
export const useProviderUpdates = create<Store>((set, get) => {
  const patch = (key: string, value: Partial<Slot>) => set(s => ({ slots: { ...s.slots, [key]: { ...s.slots[key], ...value } } }));
  return {
    slots: {},
    async check(target) {
      const key = updateTargetKey(target);
      const slot = get().slots[key];
      if (slot?.checking || slot?.updating || slot?.updated || (slot?.checkedAt && Date.now() - slot.checkedAt < UPDATE_INTERVAL)) return;
      patch(key, { checking: true });
      try {
        const report = await invoke<ProviderUpdateReport>("agent_chat_provider_update_check", { ...target });
        patch(key, { report, checkedAt: Date.now() });
      } catch {
        // Offline/unsupported remote commands must not produce update toasts.
        patch(key, { checkedAt: Date.now() });
      } finally { patch(key, { checking: false }); }
    },
    async update(target) {
      const key = updateTargetKey(target);
      if (get().slots[key]?.updating) return;
      patch(key, { updating: true, error: undefined });
      try {
        const report = await invoke<ProviderUpdateReport>("agent_chat_provider_update", { ...target });
        patch(key, { report, updated: true, checkedAt: Date.now() });
      } catch (error) {
        patch(key, { error: String(error) });
      } finally { patch(key, { updating: false }); }
    },
    dismiss(target) {
      const key = updateTargetKey(target);
      patch(key, { dismissed: get().slots[key]?.updated ? "updated" : updateIdentity(get().slots[key]?.report) });
    },
  };
});
