/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render as rtlRender, renderHook, screen, waitFor } from "@testing-library/react";
import type { ReactElement } from "react";
import { TooltipProvider } from "@/components/ui/tooltip";

const { thread, switchThread } = vi.hoisted(() => ({
  thread: { current: { account_id: "codex-work", bound: false } },
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
import { useUIStore } from "@/stores/ui-store";
import { MultiProviderModelPicker } from "./MultiProviderModelPicker";
import { useUsageLimitOccupant } from "../use-composer-strip-occupants";
import { useThreadAccount } from "../use-thread-account";

// The app mounts one TooltipProvider at the root; tests supply it the same way.
const render = (ui: ReactElement) => rtlRender(ui, { wrapper: TooltipProvider });

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
  thread.current = { account_id: "codex-work", bound: false };
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
  const side = screen.getByTestId("provider-rail-codex-codex-side");
  expect(side).toHaveAttribute("aria-disabled", "true");
  fireEvent.click(side);
  expect(useProviderAccounts.getState().picks[pickKey("codex", "t1")]).not.toBe("codex-side");
  // What picking does is the account header's tooltip, not a footer.
  expect(screen.getByTestId("provider-account-header")).toHaveTextContent("This chat starts on the account you pick.");
  fireEvent.click(personal);
  expect(useProviderAccounts.getState().picks[pickKey("codex", "t1")]).toBe("default");
  expect(switchThread).not.toHaveBeenCalled();
});

it("on a started chat, choosing another account moves it, but not while a turn runs", async () => {
  thread.current = { ...thread.current, bound: true };
  renderPicker();
  fireEvent.click(screen.getByTestId("multi-provider-model-picker-trigger"));
  await screen.findByText("Picking another account moves this chat there and keeps the conversation.");
  // No per-workspace default to set from here any more.
  expect(screen.queryByRole("menuitemcheckbox")).toBeNull();
  expect(screen.queryByText(/for new chats in this workspace/)).toBeNull();
  fireEvent.click(screen.getByTestId("provider-rail-codex-default"));
  await waitFor(() => expect(switchThread).toHaveBeenCalledWith("t1", "default"));

  cleanup();
  switchThread.mockClear();
  thread.current = { ...thread.current, account_id: "codex-work" };
  setStreaming(true);
  renderPicker();
  fireEvent.click(screen.getByTestId("multi-provider-model-picker-trigger"));
  const other = await screen.findByTestId("provider-rail-codex-default");
  expect(other).toHaveAttribute("aria-disabled", "true");
  fireEvent.click(other);
  expect(switchThread).not.toHaveBeenCalled();
  // The rail entry itself says why.
  fireEvent.focus(other);
  expect((await screen.findAllByText("Accounts can be switched once this turn finishes.")).length).toBeGreaterThan(0);
});

it("the rail's settings button opens Settings → Providers", async () => {
  const setShowSettings = vi.fn();
  const original = useUIStore.getState().setShowSettings;
  useUIStore.setState({ setShowSettings });
  try {
    renderPicker();
    fireEvent.click(screen.getByTestId("multi-provider-model-picker-trigger"));
    fireEvent.click(await screen.findByRole("button", { name: "Manage accounts" }));
    expect(setShowSettings).toHaveBeenCalledWith(true, "providers");
    await waitFor(() => expect(screen.queryByTestId("multi-provider-rail")).toBeNull());
  } finally {
    useUIStore.setState({ setShowSettings: original });
  }
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
  expect(screen.queryByTestId("provider-account-header")).toBeNull();
  expect(screen.queryByRole("button", { name: "Manage accounts" })).toBeNull();
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

it("the model list's fades follow its scroll, also after the picker reopens", async () => {
  // jsdom has no layout: 300px of rows in a 100px list, scrolled to the top.
  const overflow = (list: HTMLElement) => {
    Object.defineProperty(list, "scrollHeight", { configurable: true, value: 300 });
    Object.defineProperty(list, "clientHeight", { configurable: true, value: 100 });
    fireEvent.scroll(list);
  };
  renderPicker();
  const trigger = screen.getByTestId("multi-provider-model-picker-trigger");
  fireEvent.click(trigger);
  overflow(await screen.findByTestId("multi-provider-model-list"));
  expect(screen.getByTestId("multi-provider-model-list-bottom-fade")).toHaveAttribute("data-visible", "true");
  expect(screen.getByTestId("multi-provider-model-list-top-fade")).toHaveAttribute("data-visible", "false");

  // Closing unmounts the list; the reopened one is a new element.
  fireEvent.click(trigger);
  await waitFor(() => expect(screen.queryByTestId("multi-provider-model-list")).toBeNull());
  fireEvent.click(trigger);
  const list = await screen.findByTestId("multi-provider-model-list");
  overflow(list);
  list.scrollTop = 200;
  fireEvent.scroll(list);
  expect(screen.getByTestId("multi-provider-model-list-top-fade")).toHaveAttribute("data-visible", "true");
  expect(screen.getByTestId("multi-provider-model-list-bottom-fade")).toHaveAttribute("data-visible", "false");
});
