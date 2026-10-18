import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ProviderAccount, ProviderAccountsState } from "@/tauri/commands";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { ProviderAccountsSettings } from "./provider-accounts-settings";
import { AccountsSection } from "./usage-accounts";
import { parseQuotaKey } from "./usage-filter";
import { useProviderAccounts, withAccountPick } from "@/stores/provider-accounts-store";

const account = (over: Partial<ProviderAccount>): ProviderAccount => ({
  id: "default",
  provider: "claude",
  label: null,
  home: "/home/me/.claude",
  identity: { signed_in: true, email: "me@home.dev", organization: null, plan: null },
  login_command: "claude auth login",
  ...over,
});

let state: ProviderAccountsState;

beforeEach(() => {
  state = {
    accounts: [
      account({}),
      account({
        id: "claude-work-1",
        label: "Work",
        home: "/cfg/claude/claude-work-1",
        identity: { signed_in: false, email: null, organization: null, plan: null },
        login_command: "CLAUDE_CONFIG_DIR='/cfg/claude/claude-work-1' claude auth login",
      }),
      account({ provider: "codex", home: "/home/me/.codex" }),
    ],
    defaults: {},
  };
  useProviderAccounts.setState({ accounts: null, defaults: {}, picks: {} });
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "provider_accounts_list" || cmd === "provider_accounts_refresh") return state;
    if (cmd === "provider_account_set_default") {
      state = { ...state, defaults: { [String(args!.provider)]: String(args!.accountId) } };
      return state;
    }
    if (cmd === "provider_account_add") {
      const added = account({ id: "claude-side-2", label: String(args!.label), identity: { signed_in: false, email: null, organization: null, plan: null } });
      state = { ...state, accounts: [...state.accounts, added] };
      return added;
    }
    return null;
  });
});
afterEach(cleanup);

it("lists only this provider's accounts, flags signed-out ones, and shows their sign-in command", async () => {
  render(<ProviderAccountsSettings provider="claude" />);
  const section = await screen.findByRole("region", { name: "Accounts" });
  await within(section).findByText("Work");
  expect(within(section).getByText("Default")).toBeTruthy();
  expect(within(section).getByText("me@home.dev")).toBeTruthy();
  expect(within(section).getByText("Not signed in")).toBeTruthy();
  expect(within(section).getByText(/CLAUDE_CONFIG_DIR=/)).toBeTruthy();
  // The CLI's own sign-in is the default until another account is chosen.
  expect(within(section).getAllByText("Default for new chats")).toHaveLength(1);
});

it("makes an account the default and adds a new one", async () => {
  render(<ProviderAccountsSettings provider="claude" />);
  const section = await screen.findByRole("region", { name: "Accounts" });
  await within(section).findByText("Work");
  fireEvent.click(within(section).getByRole("button", { name: "Make default" }));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("provider_account_set_default", {
      provider: "claude",
      accountId: "claude-work-1",
      workspaceId: null,
    }),
  );

  fireEvent.click(within(section).getByRole("button", { name: "Add account" }));
  fireEvent.change(within(section).getByRole("textbox", { name: "Account name" }), { target: { value: "Side" } });
  fireEvent.click(within(section).getByRole("button", { name: "Add" }));
  await within(section).findByText("Side");
  expect(invoke).toHaveBeenCalledWith("provider_account_add", { provider: "claude", label: "Side" });
});

it("a pick for a chat that has not started rides along on its start, for its provider only", () => {
  useProviderAccounts.getState().pick("claude", "draft-1", "claude-work-1");
  const input = {
    thread_id: "draft-1",
    cwd: "/repo",
    model: null,
    resume_cursor: null,
    permission_mode: null,
    additional_directories: [],
    env: null,
    extra: { keep: 1 },
  };
  expect(withAccountPick("claude", input).extra).toEqual({ keep: 1, provider_account_id: "claude-work-1" });
  expect(withAccountPick("claude", { ...input, thread_id: "other" })).toEqual({ ...input, thread_id: "other" });
  // The draft switched to Codex: the Claude pick does not follow it.
  expect(withAccountPick("codex", input)).toEqual(input);
});

it("quota keys split into provider and account", () => {
  expect(parseQuotaKey("claude")).toEqual({ provider: "claude", accountId: "default" });
  expect(parseQuotaKey("codex@codex-side-1")).toEqual({ provider: "codex", accountId: "codex-side-1" });
});

it("the usage split shows each account with its share and plan left", () => {
  useProviderAccounts.setState({ accounts: state.accounts });
  render(
    <AccountsSection
      accounts={[
        { provider: "claude", account_id: "default", label: null, tokens: 300, cost_usd: 3, session_count: 2 },
        { provider: "claude", account_id: "claude-work-1", label: "Work", tokens: 100, cost_usd: 1, session_count: 1 },
      ]}
      quota={{
        "claude@claude-work-1": {
          windows: [{ kind: "five_hour", used_pct: 90, resets_at_ms: null, label: null, window_mins: 300 }],
          received_at_ms: 0,
        },
      }}
      metric="cost"
      onOpenLimits={() => {}}
    />,
  );
  expect(screen.getByText("Work")).toBeTruthy();
  expect(screen.getByLabelText(/75% of Claude/)).toBeTruthy();
  expect(screen.getByLabelText(/25% of Claude/)).toBeTruthy();
  expect(screen.getByText("10%")).toBeTruthy();
  expect(screen.getByText("1 session")).toBeTruthy();
});
