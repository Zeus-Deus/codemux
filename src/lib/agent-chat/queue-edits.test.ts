import { describe, expect, it } from "vitest";

import { restoreSavedQueue } from "@/components/chat/queued-turn-controls";
import { queuedMessages } from "@/components/chat/use-composer-strip-occupants";

import { applyEvent, appendUserMessage, createEmptyThreadState } from "./reducer";
import type { ChatThreadState, UserMessageItem } from "./types";

function queue(ids: string[]): ChatThreadState {
  let state = appendUserMessage(createEmptyThreadState(), "running turn", undefined, "n0");
  for (const id of ids) {
    state = applyEvent(state, {
      type: "turn_queued",
      thread_id: "t",
      queued_id: id,
      client_nonce: `nonce-${id}`,
      text: `text ${id}`,
    });
  }
  return state;
}

/** Queued ids as rendered: by seq, the transcript's order. */
function rendered(state: ChatThreadState): string[] {
  return state.messages
    .filter((m): m is UserMessageItem => m.kind === "user_message" && !!m.queued)
    .sort((a, b) => a.seq - b.seq)
    .map((m) => m.queued!.queuedId);
}

describe("queued turn edits", () => {
  it("replaces a queued bubble's text in place", () => {
    const state = applyEvent(queue(["a", "b"]), {
      type: "queued_turn_edited",
      thread_id: "t",
      queued_id: "a",
      text: "rewritten",
    });
    expect(queuedMessages(state.messages).map((m) => m.text)).toEqual(["rewritten", "text b"]);
    const unknown = applyEvent(state, {
      type: "queued_turn_edited",
      thread_id: "t",
      queued_id: "gone",
      text: "x",
    });
    expect(unknown).toBe(state);
  });

  it("reorders the queue in both the transcript and the strip", () => {
    const before = queue(["a", "b", "c"]);
    const state = applyEvent(before, {
      type: "queued_turns_reordered",
      thread_id: "t",
      queued_ids: ["c", "a", "b"],
    });
    expect(rendered(state)).toEqual(["c", "a", "b"]);
    expect(queuedMessages(state.messages).map((m) => m.queued!.queuedId)).toEqual(["c", "a", "b"]);
    // The running turn above the queue does not move.
    expect(state.messages[0]).toBe(before.messages[0]);
    // Ids that already left the queue are skipped.
    const partial = applyEvent(state, {
      type: "queued_turns_reordered",
      thread_id: "t",
      queued_ids: ["gone", "b", "c"],
    });
    expect(rendered(partial)).toEqual(["b", "a", "c"]);
  });
});

describe("restoreSavedQueue", () => {
  const row = (id: string, saved = true) => ({
    queued_id: id,
    text: `saved ${id}`,
    saved,
    client_nonce: `nonce-${id}`,
    images: id === "s1" ? [{ path: "/tmp/a.png", media_type: "image/png" }] : [],
  });

  it("adds saved turns once, as saved queued bubbles", () => {
    const empty = createEmptyThreadState();
    const state = restoreSavedQueue(empty, "t", [row("s1"), row("live", false), row("s2")]);
    const restored = queuedMessages(state.messages);
    expect(restored.map((m) => m.queued)).toEqual([
      { queuedId: "s1", saved: true },
      { queuedId: "s2", saved: true },
    ]);
    expect(restored[0].images).toEqual([{ src: "/tmp/a.png", mediaType: "image/png" }]);
    expect(restoreSavedQueue(state, "t", [row("s1"), row("s2")])).toBe(state);
  });

  it("reconciles text, order and removals with the saved queue", () => {
    const first = restoreSavedQueue(createEmptyThreadState(), "t", [row("s1"), row("s2"), row("s3")]);
    const next = restoreSavedQueue(first, "t", [
      { ...row("s3"), text: "edited elsewhere" },
      row("s1"),
    ]);
    expect(queuedMessages(next.messages).map((m) => [m.queued!.queuedId, m.text])).toEqual([
      ["s3", "edited elsewhere"],
      ["s1", "saved s1"],
    ]);
  });

  it("orders a resent saved turn behind the live turns it joined", () => {
    let state = restoreSavedQueue(createEmptyThreadState(), "t", [row("s1")]);
    state = applyEvent(state, {
      type: "turn_queued", thread_id: "t", queued_id: "live-c", client_nonce: "nonce-c", text: "c",
    });
    // Sending the saved turn behind the running one re-keys its bubble.
    state = applyEvent(state, {
      type: "turn_queued", thread_id: "t", queued_id: "live-a", client_nonce: "nonce-s1", text: "saved s1",
    });
    expect(queuedMessages(state.messages).map((m) => m.queued!.queuedId)).toEqual(["live-c", "live-a"]);
  });

  it("marks a bubble kept from before a backend restart as saved", () => {
    const live = applyEvent(createEmptyThreadState(), {
      type: "turn_queued",
      thread_id: "t",
      queued_id: "s1",
      client_nonce: "nonce-s1",
      text: "saved s1",
    });
    const restored = restoreSavedQueue(live, "t", [row("s1")]);
    expect(queuedMessages(restored.messages).map((m) => m.queued)).toEqual([
      { queuedId: "s1", saved: true },
    ]);
  });

  it("leaves a turn that was already sent under its nonce alone", () => {
    const sent = appendUserMessage(createEmptyThreadState(), "saved s1", undefined, "nonce-s1");
    expect(restoreSavedQueue(sent, "t", [row("s1")])).toBe(sent);
  });
});
