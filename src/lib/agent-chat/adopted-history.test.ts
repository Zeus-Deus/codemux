import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/tauri/commands", () => ({
  agentChatLoadAdoptedHistory: vi.fn(),
}));

import {
  agentChatLoadAdoptedHistory,
  type AdoptedHistoryPage,
  type AgentChatMessageRow,
} from "@/tauri/commands";
import { useAgentChatStore } from "@/stores/agent-chat-store";

import {
  ADOPTED_HISTORY_PAGE_SIZE,
  adoptedHistoryKey,
  getAdoptedHistory,
  loadAdoptedHistory,
  loadEarlierAdoptedHistory,
  resetAdoptedHistoryForTests,
  subscribeAdoptedHistory,
} from "./adopted-history";

const THREAD = "chat-adopted-1";
const KEY = adoptedHistoryKey(THREAD);

/** One terminal turn as the backend pages it: prompt, answer, turn end.
 *  Ids are negative and strictly increasing, like the real page. */
function turnRows(turn: number, firstId: number): AgentChatMessageRow[] {
  const turnId = `terminal-turn-${turn}`;
  return [
    {
      id: firstId,
      payload: JSON.stringify({
        type: "user_message",
        thread_id: THREAD,
        client_nonce: `terminal-nonce-${turn}`,
        text: `prompt ${turn}`,
      }),
      created_at_ms: 1_000 * turn,
    },
    {
      id: firstId + 1,
      payload: JSON.stringify({
        type: "item_completed",
        thread_id: THREAD,
        turn_id: turnId,
        item: { kind: "assistant_text", text: `answer ${turn}` },
      }),
      created_at_ms: 1_000 * turn + 1,
    },
    {
      id: firstId + 2,
      payload: JSON.stringify({
        type: "turn_completed",
        thread_id: THREAD,
        turn_id: turnId,
        status: { kind: "success" },
        usage: null,
      }),
      created_at_ms: 1_000 * turn + 2,
    },
  ];
}

const LAST_PAGE: AdoptedHistoryPage = {
  rows: [...turnRows(2, -6), ...turnRows(3, -3)],
  total: 9,
  offset: 3,
};
const FIRST_PAGE: AdoptedHistoryPage = {
  rows: turnRows(1, -9),
  total: 9,
  offset: 0,
};

function shadowMessages() {
  return useAgentChatStore.getState().threads[KEY]?.messages ?? [];
}

function userTexts() {
  return shadowMessages()
    .filter((m) => m.kind === "user_message")
    .map((m) => (m.kind === "user_message" ? m.text : ""));
}

beforeEach(() => {
  useAgentChatStore.setState({ threads: {} });
  resetAdoptedHistoryForTests();
  vi.mocked(agentChatLoadAdoptedHistory).mockReset();
});

afterEach(() => {
  resetAdoptedHistoryForTests();
});

describe("loadAdoptedHistory", () => {
  it("asks for the last page and hydrates it into the shadow key, not the thread", async () => {
    vi.mocked(agentChatLoadAdoptedHistory).mockResolvedValue(LAST_PAGE);

    await loadAdoptedHistory(THREAD, "claude");

    expect(vi.mocked(agentChatLoadAdoptedHistory)).toHaveBeenCalledWith(
      THREAD,
      null,
      ADOPTED_HISTORY_PAGE_SIZE,
    );
    const state = getAdoptedHistory(THREAD);
    expect(state.status).toBe("ready");
    expect(state.offset).toBe(3);
    expect(state.total).toBe(9);
    expect(state.historyKey).toBe(`${THREAD}::terminal-history`);
    expect(userTexts()).toEqual(["prompt 2", "prompt 3"]);
    // The live thread is untouched: the import is a separate slice.
    expect(useAgentChatStore.getState().threads[THREAD]).toBeUndefined();
    // Hydration is the reload path: nothing is streaming, no turn is live.
    const shadow = useAgentChatStore.getState().threads[KEY]!;
    expect(shadow.streaming).toBe(false);
    expect(shadow.activeTurnId).toBeNull();
  });

  it("loads once: a second call while loading or ready is a no-op", async () => {
    let resolve!: (page: AdoptedHistoryPage) => void;
    vi.mocked(agentChatLoadAdoptedHistory).mockReturnValue(
      new Promise<AdoptedHistoryPage>((r) => {
        resolve = r;
      }),
    );
    const first = loadAdoptedHistory(THREAD);
    expect(getAdoptedHistory(THREAD).status).toBe("loading");
    await loadAdoptedHistory(THREAD);
    resolve(LAST_PAGE);
    await first;
    await loadAdoptedHistory(THREAD);
    expect(vi.mocked(agentChatLoadAdoptedHistory)).toHaveBeenCalledTimes(1);
  });

  it("records a failed read as an error instead of throwing", async () => {
    vi.mocked(agentChatLoadAdoptedHistory).mockRejectedValue(
      "thread is not an adopted session",
    );
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});

    await expect(loadAdoptedHistory(THREAD)).resolves.toBeUndefined();

    const state = getAdoptedHistory(THREAD);
    expect(state.status).toBe("error");
    expect(state.error).toBe("thread is not an adopted session");
    expect(shadowMessages()).toEqual([]);
    warn.mockRestore();
  });

  it("notifies subscribers on every transition", async () => {
    vi.mocked(agentChatLoadAdoptedHistory).mockResolvedValue(LAST_PAGE);
    const seen: string[] = [];
    const unsubscribe = subscribeAdoptedHistory(() => {
      seen.push(getAdoptedHistory(THREAD).status);
    });
    await loadAdoptedHistory(THREAD);
    unsubscribe();
    expect(seen).toEqual(["loading", "ready"]);
  });
});

describe("loadEarlierAdoptedHistory", () => {
  it("prepends the previous page, re-hydrating the shadow key in order, and moves the offset", async () => {
    vi.mocked(agentChatLoadAdoptedHistory)
      .mockResolvedValueOnce(LAST_PAGE)
      .mockResolvedValueOnce(FIRST_PAGE);
    await loadAdoptedHistory(THREAD, "claude");

    await loadEarlierAdoptedHistory(THREAD, "claude");

    expect(vi.mocked(agentChatLoadAdoptedHistory)).toHaveBeenLastCalledWith(
      THREAD,
      3,
      ADOPTED_HISTORY_PAGE_SIZE,
    );
    const state = getAdoptedHistory(THREAD);
    expect(state.offset).toBe(0);
    expect(state.total).toBe(9);
    expect(state.rows.map((row) => row.id)).toEqual([
      -9, -8, -7, -6, -5, -4, -3, -2, -1,
    ]);
    expect(userTexts()).toEqual(["prompt 1", "prompt 2", "prompt 3"]);
  });

  it("does nothing at the beginning, before the first page, or while a page is in flight", async () => {
    await loadEarlierAdoptedHistory(THREAD);
    expect(vi.mocked(agentChatLoadAdoptedHistory)).not.toHaveBeenCalled();

    vi.mocked(agentChatLoadAdoptedHistory).mockResolvedValueOnce(FIRST_PAGE);
    await loadAdoptedHistory(THREAD);
    await loadEarlierAdoptedHistory(THREAD);
    expect(vi.mocked(agentChatLoadAdoptedHistory)).toHaveBeenCalledTimes(1);
  });

  it("keeps the shown rows and reports the failure when an earlier page cannot be read", async () => {
    vi.mocked(agentChatLoadAdoptedHistory)
      .mockResolvedValueOnce(LAST_PAGE)
      .mockRejectedValueOnce(new Error("session file vanished"));
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await loadAdoptedHistory(THREAD);

    await loadEarlierAdoptedHistory(THREAD);

    const state = getAdoptedHistory(THREAD);
    expect(state.status).toBe("ready");
    expect(state.offset).toBe(3);
    expect(state.loadingEarlier).toBe(false);
    expect(state.error).toBe("session file vanished");
    expect(userTexts()).toEqual(["prompt 2", "prompt 3"]);
    warn.mockRestore();
  });
});
