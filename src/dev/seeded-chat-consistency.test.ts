import { beforeAll, expect, it } from "vitest";
import type { AppStateSnapshot, PaneNodeSnapshot } from "@/tauri/types";
import { MOCK_SECOND_CHAT_THREAD_ID } from "./mock-fixtures";

/**
 * The real backend derives a chat pane's sidebar status, its provider and its
 * liveness probe from one thread binding, so they cannot disagree. The mock
 * seeds them from separate fixtures; these checks keep the seeds in step so
 * `npm run dev` does not show states the app cannot reach.
 */

/** Bound threads the mock deliberately persists no session record for. Any
 *  other bound pane without a record fails, so a forgotten record cannot
 *  quietly exempt its pane from the provider check. */
const RECORDLESS_THREADS = new Set([MOCK_SECOND_CHAT_THREAD_ID]);

type Invoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;
type ChatPane = Extract<PaneNodeSnapshot, { kind: "agent_chat" }>;

let invoke: Invoke;
let state: AppStateSnapshot;
let seededChats: ChatPane[];

function chatPanes(node: PaneNodeSnapshot): ChatPane[] {
  if (node.kind === "agent_chat") return [node];
  if (node.kind === "split") return node.children.flatMap(chatPanes);
  return [];
}

beforeAll(async () => {
  await import("./tauri-mock");
  invoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: Invoke } }).__TAURI_INTERNALS__.invoke;
  state = (await invoke("get_app_state")) as AppStateSnapshot;
  // Unbound panes are out of scope: liveness needs a thread id, and the one
  // seeded `working` unbound pane (`ws-codemux-chat-live`) is the fresh-pane
  // streaming demo, which has no thread to disagree with.
  seededChats = state.workspaces
    .flatMap((w) => w.surfaces.flatMap((s) => chatPanes(s.root)))
    .filter((p) => p.thread_id !== null);
});

it("seeds every bound chat pane with the provider its session record names", async () => {
  let checked = 0;
  for (const pane of seededChats) {
    const record = (await invoke("agent_chat_get_session", { threadId: pane.thread_id })) as
      | { provider: string }
      | null;
    if (!record) {
      expect(RECORDLESS_THREADS).toContain(pane.thread_id);
      continue;
    }
    checked += 1;
    expect({ thread: pane.thread_id, provider: pane.provider }).toEqual({
      thread: pane.thread_id,
      provider: record.provider,
    });
  }
  expect(checked).toBeGreaterThan(1);
});

it("reports a live turn for exactly the bound chat panes the sidebar shows as working", async () => {
  for (const pane of seededChats) {
    const working = state.pane_statuses[pane.pane_id] === "working";
    const live = await invoke("agent_chat_turn_active", {
      provider: pane.provider,
      threadId: pane.thread_id,
    });
    expect({ thread: pane.thread_id, live }).toEqual({ thread: pane.thread_id, live: working });
  }
});
