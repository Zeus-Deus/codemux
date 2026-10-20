import { useEffect } from "react";
import {
  accountCatalogKey,
  selectCapabilities,
  selectError,
  useProviderCapabilities,
} from "@/stores/provider-capabilities-store";
import { DEFAULT_ACCOUNT_ID } from "@/stores/provider-accounts-store";
import type { AgentChatProviderKind, ProviderChatCapabilities } from "@/tauri/types";
import { useThreadAccount } from "./use-thread-account";

export interface NativeCapabilities {
  caps: ProviderChatCapabilities | null;
  error: string | null;
  /** The extra Codex account the catalog belongs to, if any. */
  accountId: string | null;
}

/**
 * `provider`'s model catalog as `threadId` sees it. An extra Codex account
 * signs in on its own, so a chat running (or about to start) on one gets
 * that account's catalog and sign-in errors. While the chat's account or
 * that catalog is still unknown there is no catalog: another account's
 * would let the composer offer its models or normalize settings to it.
 */
export function useNativeCapabilities(
  provider: AgentChatProviderKind,
  threadId: string | null | undefined,
): NativeCapabilities {
  const codex = useThreadAccount("codex", threadId, provider === "codex");
  const accountId =
    provider === "codex" && codex.multi && codex.current && codex.current.id !== DEFAULT_ACCOUNT_ID
      ? codex.current.id
      : null;
  const catalog = useProviderCapabilities((s) =>
    accountId ? s.accountCatalogs[accountCatalogKey("codex", accountId)] : undefined,
  );
  const caps = useProviderCapabilities((s) => selectCapabilities(s, provider));
  const error = useProviderCapabilities((s) => selectError(s, provider));
  useEffect(() => {
    if (accountId && !catalog) void useProviderCapabilities.getState().refreshAccount("codex", accountId);
  }, [accountId, catalog]);
  // A failed read falls back to the provider's catalog rather than none.
  if (provider === "codex" && codex.pending) return { caps: null, error: null, accountId: null };
  if (accountId) return { caps: catalog?.caps ?? null, error: catalog?.error ?? null, accountId };
  return { caps, error, accountId: null };
}
