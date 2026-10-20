/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";

const { thread, switchThread } = vi.hoisted(() => ({
  thread: { current: { account_id: "codex-work", bound: false, workspace_id: "ws-1", workspace_default: null as string | null } },
  switchThread: vi.fn(async (_threadId: string, accountId: string) => {
    thread.current = { ...thread.current, account_id: accountId };
    return { account_id: accountId, carried: true };
  }),
}));

vi.mock("@/tauri/commands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/tauri/commands")>()),
  listChatProviderCapabilities: vi.fn(() => new Promise(() => {})),
  agentChatProviderHealth: vi.fn(async (provider) => ({ provider, status: "ready", installed: true, message: null, version: null })),
  providerAccountsList: vi.fn(async () => ({ accounts: ACCOUNTS, defaults: {} })),
  providerAccountsRefresh: vi.fn(async () => ({ accounts: ACCOUNTS, defaults: {} })),
  providerAccountThread: vi.fn(async () => ({ ...thread.current })),
  providerAccountSwitchThread: switchThread,
}));
vi.mock("@/tauri/custom-acp", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/tauri/custom-acp")>()),
  acpAgents: vi.fn(() => new Promise(() => {})),
}));

import { listChatProviderCapabilities, type ProviderAccount } from "@/tauri/commands";
import type { ChatModelInfo } from "@/tauri/types";
import type { UsageLimitState } from "@/lib/agent-chat/types";
import { resetProviderCapabilities, useProviderCapabilities } from "@/stores/provider-capabilities-store";
import { pickKey, useProviderAccounts } from "@/stores/provider-accounts-store";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { MultiProviderModelPicker } from "./MultiProviderModelPicker";
import { useUsageLimitOccupant } from "../use-composer-strip-occupants";
import { useThreadAccount } from "../use-thread-account";

const account = (over: Partial<ProviderAccount>): ProviderAccount => ({
  id: "default",
  provider: "codex",
  label: null,
  home: "/home/me/.codex",
  identity: { signed_in: true, email: "me@home.dev", organization: null, plan: "ChatGPT Plus" },
  login_command: "codex login",
  ...over,
});

const ACCOUNTS: ProviderAccount[] = [
  account({}),
  account({
    id: "codex-work",
    label: "Work",
    home: "/cfg/codex/codex-work",
    identity: { signed_in: true, email: "me@acme.dev", organization: "Acme", plan: "ChatGPT Pro" },
  }),
  account({
    id: "codex-side",
    label: "Side project",
    home: "/cfg/codex/codex-side",
    identity: { signed_in: false, email: null, organization: null, plan: null },
  }),
];

const model = (id: string, label: string): ChatModelInfo => ({
  id,
  label,
  description: null,
  effort_levels: [],
  default_effort: null,
  prompt_injected_effort_levels: [],
  context_window_options: [],
  supports_adaptive_thinking: false,
  supports_thinking_toggle: false,
  supports_fast_mode: false,
  supports_images: false,
  sub_provider: null,
  is_free: false,
});

function setStreaming(streaming: boolean) {
  const store = useAgentChatStore.getState();
  store.ensureThread("t1");
  useAgentChatStore.setState((s) => ({ threads: { ...s.threads, t1: { ...s.threads.t1, streaming } } }));
}

beforeEach(() => {
  vi.mocked(listChatProviderCapabilities).mockReset().mockImplementation(() => new Promise(() => {}));
  resetProviderCapabilities();
  setStreaming(false);
  thread.current = { account_id: "codex-work", bound: false, workspace_id: "ws-1", workspace_default: null };
  switchThread.mockClear();
  useProviderAccounts.setState({ accounts: ACCOUNTS, defaults: {}, picks: {}, pickerRequest: null });
  useProviderCapabilities.setState({
    codex: {
      models: [model("gpt-5.4", "gpt-5.4"), model("gpt-5.4-mini", "gpt-5.4-mini")],
      effort_granularity: "per_session",
      effort_label_map: {},
      permission_modes: [],
      default_permission_mode: null,
      permission_granularity: "per_session",
    },
  } as never);
});
afterEach(cleanup);

const renderPicker = () =>
  render(
    <MultiProviderModelPicker
      hermesThreadId="t1"
      provider="codex"
      model="gpt-5.4"
      onProviderModelChange={() => {}}
    />,
  );

it("badges the model chip with the chat's account and names it in the hover reveal, never the email", async () => {
  renderPicker();
  const trigger = screen.getByTestId("multi-provider-model-picker-trigger");
  await waitFor(() =>
    expect(screen.getByTestId("multi-provider-model-picker-reveal")).toHaveTextContent(/^Work·$/),
  );
  expect(trigger).toHaveTextContent("W");
  fireEvent.click(trigger);
  await screen.findByTestId("provider-rail-codex-codex-work");
  expect(screen.getByTestId("provider-account-header")).toHaveTextContent("WorkChatGPT ProCodex");
  expect(document.body.textContent).not.toMatch(/@acme\.dev|@home\.dev/);
});

it("one rail entry per account; a draft's choice is its pick, and a signed-out account can't be chosen", async () => {
  renderPicker();
  fireEvent.click(screen.getByTestId("multi-provider-model-picker-trigger"));
  const personal = await screen.findByTestId("provider-rail-codex-default");
  expect(screen.queryByTestId("provider-rail-codex")).toBeNull();
  expect(screen.getByTestId("provider-rail-codex-codex-side")).toBeDisabled();
  expect(screen.getByText("This chat starts on the account you pick.")).toBeInTheDocument();
  fireEvent.click(personal);
  expect(useProviderAccounts.getState().picks[pickKey("codex", "t1")]).toBe("default");
  expect(switchThread).not.toHaveBeenCalled();
});

it("on a started chat, choosing another account moves it, but not while a turn runs", async () => {
  thread.current = { ...thread.current, bound: true };
  renderPicker();
  fireEvent.click(screen.getByTestId("multi-provider-model-picker-trigger"));
  await screen.findByText("Picking another account moves this chat there and keeps the conversation.");
  expect(screen.getByRole("menuitemcheckbox", { name: "Use Work for new chats in this workspace" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Manage accounts…" })).toBeInTheDocument();
  fireEvent.click(screen.getByTestId("provider-rail-codex-default"));
  await waitFor(() => expect(switchThread).toHaveBeenCalledWith("t1", "default"));

  cleanup();
  switchThread.mockClear();
  thread.current = { ...thread.current, account_id: "codex-work" };
  setStreaming(true);
  renderPicker();
  fireEvent.click(screen.getByTestId("multi-provider-model-picker-trigger"));
  expect(await screen.findByTestId("provider-rail-codex-default")).toBeDisabled();
});

it("an extra account lists the models its own sign-in offers", async () => {
  // The CLI login is signed out; Work is signed in.
  const notSignedIn = "codex_not_authenticated: Codex is not signed in";
  const workCaps = { ...useProviderCapabilities.getState().codex!, models: [model("gpt-work", "GPT Work")] };
  vi.mocked(listChatProviderCapabilities).mockImplementation(async (provider, _force, accountId) => {
    if (provider !== "codex") return new Promise(() => {});
    if (accountId === "codex-work") return workCaps;
    throw new Error(notSignedIn);
  });
  useProviderCapabilities.setState({ codex: null, codexError: notSignedIn } as never);
  thread.current = { ...thread.current, bound: true };
  renderPicker();
  fireEvent.click(screen.getByTestId("multi-provider-model-picker-trigger"));

  expect(await screen.findByText("GPT Work")).toBeInTheDocument();
  expect(screen.queryByText(/not signed in/i)).toBeNull();
  expect(listChatProviderCapabilities).toHaveBeenCalledWith("codex", false, "codex-work");
});

it("a single account adds nothing", async () => {
  useProviderAccounts.setState({ accounts: [ACCOUNTS[0]] });
  renderPicker();
  const trigger = screen.getByTestId("multi-provider-model-picker-trigger");
  expect(trigger).not.toHaveTextContent("W");
  expect(screen.queryByTestId("provider-account-badge")).toBeNull();
  fireEvent.click(trigger);
  await screen.findByTestId("provider-rail-codex");
  expect(screen.queryByTestId("provider-account-footer")).toBeNull();
});

it("the usage-limit notice offers the switch, and Choose account… opens the picker on that provider", async () => {
  thread.current = { ...thread.current, bound: true };
  // Side project is signed out, so Default is the one account to switch to.
  const limit: UsageLimitState = { provider: "codex", resetsAtMs: Date.now() + 3_600_000, autoResumeAtMs: null, window: "five_hour", at: Date.now() };
  const { result } = renderHook(() =>
    useUsageLimitOccupant({ usageLimit: limit, threadId: "t1", streaming: false, onResume: async () => {}, onCancel: async () => {} }),
  );
  await waitFor(() => expect(result.current?.summary.action?.label).toBe("Switch to Default"));
  expect(result.current?.summary.label).toBe("Work hit its limit");
  expect(result.current?.summary.secondaryAction?.label).toBe("Choose account…");
  // Waiting on this account moves to the second row.
  expect(result.current?.rows[1].label).toBe("Or stay on Work");
  expect(result.current?.rows[1].action?.label).toBe("Try now");

  act(() => result.current?.summary.secondaryAction?.onClick());
  expect(useProviderAccounts.getState().pickerRequest).toMatchObject({ provider: "codex", threadId: "t1" });

  act(() => result.current?.summary.action?.onClick());
  await waitFor(() => expect(switchThread).toHaveBeenCalledWith("t1", "default"));
});

it("a switch made from one view moves every view of the chat", async () => {
  thread.current = { ...thread.current, bound: true };
  // The limit notice and the model chip each hold their own view.
  const { result } = renderHook(() => ({
    strip: useThreadAccount("codex", "t1"),
    chip: useThreadAccount("codex", "t1"),
  }));
  await waitFor(() => expect(result.current.chip.current?.id).toBe("codex-work"));
  await act(async () => {
    await result.current.strip.choose(ACCOUNTS[0]);
  });
  await waitFor(() => expect(result.current.chip.current?.id).toBe("default"));
});
