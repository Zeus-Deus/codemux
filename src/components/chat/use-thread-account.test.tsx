import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, renderHook } from "@testing-library/react";

type Lookup = { account_id: string; bound: boolean };

const { lookups, switchThread } = vi.hoisted(() => ({
  // Each binding read waits until the test answers it.
  lookups: [] as { threadId: string; resolve: (value: Lookup) => void }[],
  switchThread: vi.fn(async (_threadId: string, accountId: string) => ({ account_id: accountId, carried: true })),
}));

vi.mock("@/tauri/commands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/tauri/commands")>()),
  providerAccountThread: vi.fn(
    (threadId: string) => new Promise<Lookup>((resolve) => lookups.push({ threadId, resolve })),
  ),
  providerAccountSwitchThread: switchThread,
}));

import type { ProviderAccount } from "@/tauri/commands";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { pickKey, useProviderAccounts } from "@/stores/provider-accounts-store";
import { useThreadAccount } from "./use-thread-account";

const account = (id: string, label: string | null): ProviderAccount => ({
  id,
  provider: "codex",
  label,
  home: `/cfg/${id}`,
  identity: { signed_in: true, email: null, organization: null, plan: null },
  login_command: "codex login",
});

const DEFAULT = account("default", null);
const WORK = account("codex-work", "Work");

const bound = (account_id: string): Lookup => ({ account_id, bound: true });

/** Answer the oldest unanswered read of `threadId`. */
async function answer(threadId: string, value: Lookup) {
  const index = lookups.findIndex((l) => l.threadId === threadId);
  expect(index).toBeGreaterThanOrEqual(0);
  const [lookup] = lookups.splice(index, 1);
  await act(async () => lookup.resolve(value));
}

beforeEach(() => {
  lookups.length = 0;
  switchThread.mockClear();
  // Account options are already cached; only the binding is unknown.
  useProviderAccounts.setState({ accounts: [DEFAULT, WORK], defaults: {}, picks: {}, bindingRevisions: {} });
});

afterEach(cleanup);

it("does not pick or switch until the chat's binding is known", async () => {
  const { result } = renderHook(() => useThreadAccount("codex", "chat-a"));

  await act(() => result.current.choose(WORK));
  expect(useProviderAccounts.getState().picks[pickKey("codex", "chat-a")]).toBeUndefined();
  expect(switchThread).not.toHaveBeenCalled();
  expect(result.current.ready).toBe(false);

  await answer("chat-a", bound("default"));
  expect(result.current.ready).toBe(true);
  await act(() => result.current.choose(WORK));
  expect(switchThread).toHaveBeenCalledWith("chat-a", "codex-work");
  expect(useProviderAccounts.getState().picks[pickKey("codex", "chat-a")]).toBeUndefined();
});

it("ignores a late answer for the chat it showed before", async () => {
  const { result, rerender } = renderHook(({ thread }) => useThreadAccount("codex", thread), {
    initialProps: { thread: "chat-a" },
  });
  rerender({ thread: "chat-b" });
  await answer("chat-b", bound("default"));
  await answer("chat-a", bound("codex-work"));

  expect(result.current.current?.id).toBe("default");
  expect(result.current.thread?.account_id).toBe("default");
});

it("ignores a read begun before a switch that answers after it", async () => {
  const { result } = renderHook(() => useThreadAccount("codex", "chat-a"));
  await answer("chat-a", bound("default"));

  // A refresh starts, then the chat moves to Work before it answers.
  act(() => result.current.refreshIdentities());
  await act(() => result.current.choose(WORK));
  // The read after the move answers first, the stale one last.
  const stale = lookups.shift()!;
  await answer("chat-a", bound("codex-work"));
  await act(async () => stale.resolve(bound("default")));

  expect(result.current.current?.id).toBe("codex-work");
});

const unbound = (account_id: string): Lookup => ({ account_id, bound: false });

function setStreaming(threadId: string, streaming: boolean) {
  useAgentChatStore.getState().ensureThread(threadId);
  useAgentChatStore.setState((s) => ({ threads: { ...s.threads, [threadId]: { ...s.threads[threadId], streaming } } }));
}

it("a draft that just started is not a draft to pick for", async () => {
  const { result } = renderHook(() => useThreadAccount("codex", "draft-a"));
  await answer("draft-a", unbound("default"));
  expect(result.current.ready).toBe(true);

  // The first send starts the session on Default; its binding read is pending.
  act(() => setStreaming("draft-a", true));
  expect(result.current.ready).toBe(false);
  await act(() => result.current.choose(WORK));
  // Nor once that turn ends before a read confirms the binding.
  act(() => setStreaming("draft-a", false));
  expect(result.current.ready).toBe(false);
  await act(() => result.current.choose(WORK));
  expect(useProviderAccounts.getState().picks[pickKey("codex", "draft-a")]).toBe("default");
  expect(switchThread).not.toHaveBeenCalled();

  // Only the read begun after the run answers.
  const reads = lookups.splice(0);
  await act(async () => reads[reads.length - 1].resolve(bound("default")));
  expect(result.current.ready).toBe(true);
  await act(() => result.current.choose(WORK));
  expect(switchThread).toHaveBeenCalledWith("draft-a", "codex-work");
});

it("another chat's finished move does not drop this chat's answer", async () => {
  let finish: () => void = () => {};
  switchThread.mockImplementationOnce(
    (_threadId: string, accountId: string) =>
      new Promise((resolve) => (finish = () => resolve({ account_id: accountId, carried: true }))),
  );
  const { result, rerender } = renderHook(({ thread }) => useThreadAccount("codex", thread), {
    initialProps: { thread: "chat-a" },
  });
  await answer("chat-a", bound("default"));
  let moving: Promise<void> = Promise.resolve();
  act(() => {
    moving = result.current.choose(WORK);
  });
  rerender({ thread: "chat-b" });
  await act(async () => {
    finish();
    await moving;
  });
  await answer("chat-b", bound("default"));

  expect(result.current.ready).toBe(true);
  expect(result.current.thread?.account_id).toBe("default");
});

it("a second view of a draft never overrides its explicit pick", async () => {
  // A catalog reader and the picker each hold a view; the reader answers first.
  const { result } = renderHook(() => ({
    reader: useThreadAccount("codex", "draft-b"),
    picker: useThreadAccount("codex", "draft-b"),
  }));
  await answer("draft-b", unbound("default"));
  await answer("draft-b", unbound("default"));
  await act(() => result.current.picker.choose(DEFAULT));

  // The app-wide default changes to Work; both views re-read.
  act(() => result.current.reader.refreshIdentities());
  await answer("draft-b", unbound("codex-work"));
  act(() => result.current.picker.refreshIdentities());
  await answer("draft-b", unbound("codex-work"));

  expect(useProviderAccounts.getState().picks[pickKey("codex", "draft-b")]).toBe("default");
});

it("refreshing one view of a chat re-reads every view of it", async () => {
  // The picker's header and a catalog reader each hold a view.
  const { result } = renderHook(() => ({
    header: useThreadAccount("codex", "chat-c"),
    catalog: useThreadAccount("codex", "chat-c"),
  }));
  await answer("chat-c", bound("default"));
  await answer("chat-c", bound("default"));

  // Another client moved the idle chat to Work; opening the picker refreshes.
  act(() => result.current.header.refreshIdentities());
  while (lookups.some((l) => l.threadId === "chat-c")) await answer("chat-c", bound("codex-work"));

  expect(result.current.header.current?.id).toBe("codex-work");
  expect(result.current.catalog.current?.id).toBe("codex-work");
});
