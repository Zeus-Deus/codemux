import { beforeEach, expect, it, vi } from "vitest";

vi.mock("@/tauri/commands", () => ({ providerAccountsRefresh: vi.fn() }));

import { providerAccountsRefresh, type ProviderAccountsState } from "@/tauri/commands";
import { useProviderAccounts } from "./provider-accounts-store";

beforeEach(() => {
  vi.mocked(providerAccountsRefresh).mockReset();
  useProviderAccounts.setState({ accounts: null, defaults: {} });
});

it("shares an identity probe between setup and account settings", async () => {
  let resolve!: (state: ProviderAccountsState) => void;
  vi.mocked(providerAccountsRefresh).mockReturnValue(new Promise((done) => { resolve = done; }));
  const setup = useProviderAccounts.getState().refresh();
  const settings = useProviderAccounts.getState().refresh();
  expect(providerAccountsRefresh).toHaveBeenCalledTimes(1);
  const state = { accounts: [], defaults: { claude: "work" } };
  resolve(state);
  await Promise.all([setup, settings]);
  expect(useProviderAccounts.getState()).toMatchObject(state);
});

it("allows a retry after an identity probe fails", async () => {
  vi.mocked(providerAccountsRefresh).mockRejectedValueOnce(new Error("test probe failed"));
  await expect(useProviderAccounts.getState().refresh()).rejects.toThrow("test probe failed");
  vi.mocked(providerAccountsRefresh).mockResolvedValueOnce({ accounts: [], defaults: {} });
  await useProviderAccounts.getState().refresh();
  expect(providerAccountsRefresh).toHaveBeenCalledTimes(2);
  expect(useProviderAccounts.getState().accounts).toEqual([]);
});
