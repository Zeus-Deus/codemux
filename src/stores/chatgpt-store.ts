import { create } from "zustand";
import {
  getChatGptStatus, startChatGptLogin, cancelChatGptLogin, disconnectChatGpt,
  acknowledgeChatGptWelcome, getLocalWorkbench, setLocalWorkbench,
  type ChatGptStatus,
} from "@/tauri/chatgpt";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { invalidateCodexCapabilities, useProviderCapabilities } from "@/stores/provider-capabilities-store";
import { useProviderHealth } from "@/stores/provider-health-store";

interface ChatGptStore {
  status: ChatGptStatus | null;
  ready: boolean;
  loading: boolean;
  busy: boolean;
  error: string | null;
  subscriptionError: string | null;
  localMode: boolean;
  cloudLogin: boolean;
  bootstrap: () => Promise<void>;
  refresh: () => Promise<void>;
  start: (profileId: string | null) => Promise<void>;
  cancel: () => Promise<void>;
  disconnect: () => Promise<void>;
  acknowledgeWelcome: () => Promise<boolean>;
  enterLocal: () => Promise<void>;
}
let epoch = 0;
let preferenceEpoch = 0;
let bootstrapFlight: Promise<void> | null = null;
let statusReadFlight: Promise<void> | null = null;
let statusReadQueued = false;
let loginIntent = false;
const message = (error: unknown) => error instanceof Error ? error.message : String(error);

function activeAccountReady(status: ChatGptStatus | null): boolean {
  return status?.phase === "connected" || !!status?.profiles.some(
    (profile) => profile.id === status.activeProfileId && profile.connected,
  );
}
function acceptStatus(status: ChatGptStatus) {
  const previous = useChatGptStore.getState().status;
  useChatGptStore.setState({ status, error: null });
  if ((!previous && status.activeProfileId !== null) ||
      (previous && (previous.activeProfileId !== status.activeProfileId ||
        activeAccountReady(previous) !== activeAccountReady(status)))) {
    // Persisted catalogs are not account-qualified, including on first boot.
    invalidateCodexCapabilities();
  }
  if (loginIntent && status.phase !== "pending") {
    loginIntent = false;
    if (status.phase === "connected") {
      // Completing explicit sign-in is discovery intent. Initial bootstrap is not.
      void useProviderHealth.getState().refresh("codex", { force: true });
      void useProviderCapabilities.getState().refresh("codex");
    }
  }
}
function flushQueuedStatusRead() {
  if (!statusReadQueued || statusReadFlight || useChatGptStore.getState().busy) return;
  statusReadQueued = false;
  void useChatGptStore.getState().refresh();
}
async function mutate(operation: () => Promise<ChatGptStatus>) {
  if (isRemoteClient() || useChatGptStore.getState().busy) return false;
  epoch += 1;
  useChatGptStore.setState({ busy: true, error: null });
  try {
    acceptStatus(await operation());
    return true;
  } catch (error) {
    useChatGptStore.setState({ error: message(error) });
    return false;
  } finally {
    useChatGptStore.setState({ busy: false });
    flushQueuedStatusRead();
  }
}

export const useChatGptStore = create<ChatGptStore>((set, get) => ({
  status: null, ready: false, loading: false, busy: false, error: null,
  subscriptionError: null, localMode: false, cloudLogin: false,
  bootstrap: () => {
    if (bootstrapFlight) return bootstrapFlight;
    if (isRemoteClient()) { set({ ready: true }); return Promise.resolve(); }
    const requestPreferenceEpoch = preferenceEpoch;
    set({ loading: true });
    const flight = Promise.all([get().refresh(), getLocalWorkbench()])
      .then(([, localMode]) => {
        if (requestPreferenceEpoch === preferenceEpoch) set({ localMode });
      }).catch((error) => {
        if (requestPreferenceEpoch === preferenceEpoch) set({ error: message(error) });
      }).finally(() => {
        set({ ready: true, loading: false });
        if (bootstrapFlight === flight) bootstrapFlight = null;
      });
    bootstrapFlight = flight;
    return flight;
  },
  refresh: () => {
    if (isRemoteClient()) return Promise.resolve();
    if (get().busy || statusReadFlight) {
      statusReadQueued = true;
      return statusReadFlight ?? Promise.resolve();
    }
    const requestEpoch = epoch;
    const flight = (async () => {
      try {
        const status = await getChatGptStatus();
        if (requestEpoch === epoch) acceptStatus(status);
      } catch (error) {
        if (requestEpoch === epoch) set({ error: message(error) });
      }
    })().finally(() => {
      if (statusReadFlight === flight) statusReadFlight = null;
      flushQueuedStatusRead();
    });
    statusReadFlight = flight;
    return flight;
  },
  start: async (profileId) => {
    if (isRemoteClient() || get().busy || get().status?.phase === "pending") return;
    loginIntent = true;
    if (!await mutate(() => startChatGptLogin(profileId))) loginIntent = false;
  },
  cancel: async () => {
    const attemptId = get().status?.attemptId;
    if (attemptId) {
      loginIntent = false;
      await mutate(() => cancelChatGptLogin(attemptId));
    }
  },
  disconnect: async () => { loginIntent = false; await mutate(disconnectChatGpt); },
  acknowledgeWelcome: () => mutate(acknowledgeChatGptWelcome),
  enterLocal: async () => {
    if (isRemoteClient() || get().busy || get().status?.phase === "pending") return;
    preferenceEpoch += 1;
    set({ busy: true, error: null });
    try {
      await setLocalWorkbench(true);
      set({ localMode: true, cloudLogin: false });
    } catch (error) { set({ error: message(error) }); }
    finally { set({ busy: false }); flushQueuedStatusRead(); }
  },
}));
