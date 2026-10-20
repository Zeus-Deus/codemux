import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, renderHook, waitFor } from "@testing-library/react";

const { lookupFails } = vi.hoisted(() => ({ lookupFails: { current: false } }));

vi.mock("@/tauri/commands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/tauri/commands")>()),
  providerAccountThread: vi.fn(async () => {
    if (lookupFails.current) throw new Error("web-remote: not connected");
    return { account_id: "codex-work", bound: true, workspace_id: "ws", workspace_default: null };
  }),
  listChatProviderCapabilities: vi.fn(async (_provider: string, _force?: boolean, accountId?: string) => {
    if (accountId === "codex-work") return { ...CAPS, models: [{ ...MODEL, id: "gpt-work", supports_images: true }] };
    throw new Error("codex_not_authenticated: Codex is not signed in");
  }),
}));

import type { ProviderAccount } from "@/tauri/commands";
import type { ChatModelInfo, ProviderChatCapabilities } from "@/tauri/types";
import { resetProviderCapabilities, useProviderCapabilities } from "@/stores/provider-capabilities-store";
import { useProviderAccounts } from "@/stores/provider-accounts-store";
import { useNativeCapabilities } from "./use-native-capabilities";

const MODEL: ChatModelInfo = {
  id: "gpt-cli",
  label: "GPT",
  description: null,
  effort_levels: ["low", "high"],
  default_effort: "high",
  prompt_injected_effort_levels: [],
  context_window_options: [],
  supports_adaptive_thinking: false,
  supports_thinking_toggle: false,
  supports_fast_mode: false,
  supports_images: false,
  sub_provider: null,
  is_free: false,
};

const CAPS = {
  models: [MODEL],
  effort_granularity: "per_session",
  effort_label_map: {},
  permission_modes: [],
  default_permission_mode: null,
  permission_granularity: "per_session",
} as unknown as ProviderChatCapabilities;

const account = (id: string, label: string | null): ProviderAccount => ({
  id,
  provider: "codex",
  label,
  home: `/cfg/${id}`,
  identity: { signed_in: true, email: null, organization: null, plan: null },
  login_command: "codex login",
});

beforeEach(() => {
  lookupFails.current = false;
  resetProviderCapabilities();
  useProviderAccounts.setState({ accounts: [account("default", null), account("codex-work", "Work")], picks: {} });
});
afterEach(cleanup);

it("a chat on an extra account sees that account's catalog, not the CLI sign-in's error", async () => {
  // The CLI sign-in is signed out.
  useProviderCapabilities.setState({ codex: null, codexError: "codex_not_authenticated: Codex is not signed in" });
  const { result } = renderHook(() => useNativeCapabilities("codex", "t1"));

  await waitFor(() => expect(result.current.caps?.models[0].id).toBe("gpt-work"));
  expect(result.current.caps?.models[0].supports_images).toBe(true);
  expect(result.current.error).toBeNull();
});

it("a chat on the CLI sign-in keeps the provider's catalog", () => {
  useProviderAccounts.setState({ accounts: [account("default", null)] });
  useProviderCapabilities.setState({ codex: CAPS, codexError: null });
  const { result } = renderHook(() => useNativeCapabilities("codex", "t1"));
  expect(result.current.caps).toBe(CAPS);
});

it("offers no catalog until the chat's account and its catalog are known", async () => {
  // The CLI sign-in's catalog is loaded, but the chat runs on Work.
  useProviderCapabilities.setState({ codex: CAPS, codexError: null });
  const seen: (string | undefined)[] = [];
  const { result } = renderHook(() => {
    const native = useNativeCapabilities("codex", "t1");
    seen.push(native.caps?.models[0].id);
    return native;
  });
  await waitFor(() => expect(result.current.caps?.models[0].id).toBe("gpt-work"));
  expect(seen).not.toContain("gpt-cli");
});

it("a failed account read falls back to the provider's catalog instead of none", async () => {
  lookupFails.current = true;
  useProviderCapabilities.setState({ codex: CAPS, codexError: null });
  const { result } = renderHook(() => useNativeCapabilities("codex", "t1"));
  await waitFor(() => expect(result.current.caps).toBe(CAPS));
});
