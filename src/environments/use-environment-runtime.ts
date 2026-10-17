import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

import { isRemoteClient } from "@/components/remote/is-remote-client";
import { DeviceRegistry } from "@/remote/device-registry";
import { useAuthStore } from "@/stores/auth-store";

import { NativeIrohDialer, nativeAccountFetch } from "./native";
import { startEnvironmentRuntime, type EnvironmentRuntime } from "./runtime";
import { useEnvironmentsStore, type EnvironmentDescriptor } from "./store";

/** `nativeAccountFetch` forwards only the path; the host is never contacted. */
const ACCOUNT_PROXY_BASE = "https://account.codemux.invalid";

/**
 * Desktop: while signed into an account, keep live connections to the
 * account's other machines. Web remote clients drive one desktop and have no
 * account token of their own, so they skip this.
 */
export function useEnvironmentRuntime(signedIn: boolean): void {
  const userId = useAuthStore((state) => state.user?.id);
  useEffect(() => {
    if (!userId || !signedIn || isRemoteClient() || !("__TAURI_INTERNALS__" in window)) return;
    let runtime: EnvironmentRuntime | null = null;
    let cancelled = false;
    void invoke<EnvironmentDescriptor | null>("environment_info")
      .then((self) => {
        if (cancelled || !self?.id) return;
        useEnvironmentsStore.getState().setSelf(self);
        runtime = startEnvironmentRuntime({
          registry: new DeviceRegistry({
            baseUrl: ACCOUNT_PROXY_BASE,
            fetchImpl: nativeAccountFetch,
          }),
          dialer: new NativeIrohDialer(),
          selfId: self.id,
        });
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      if (runtime) runtime.stop();
      else useEnvironmentsStore.getState().reset();
    };
  }, [signedIn, userId]);
}
