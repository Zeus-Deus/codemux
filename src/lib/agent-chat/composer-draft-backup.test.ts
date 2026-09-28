import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { useAgentChatStore } from "@/stores/agent-chat-store";
import {
  flushComposerDraftBackupForTests,
  resetComposerDraftBackupForTests,
  restoredComposerDraft,
} from "./composer-draft-backup";

const KEY = "codemux:composer-drafts";

function saved(): Record<string, string> | null {
  const raw = window.sessionStorage.getItem(KEY);
  return raw ? JSON.parse(raw) : null;
}

beforeEach(() => {
  window.sessionStorage.clear();
  resetComposerDraftBackupForTests();
  useAgentChatStore.setState({ threads: {} });
});

afterEach(() => {
  resetComposerDraftBackupForTests();
  window.sessionStorage.clear();
});

describe("composer draft backup", () => {
  it("saves unsent composer text per thread", () => {
    const store = useAgentChatStore.getState();
    store.setInputDraft("t1", "half a thought");
    store.setInputDraft("t2", "another");
    flushComposerDraftBackupForTests();
    expect(saved()).toEqual({ t1: "half a thought", t2: "another" });
  });

  it("forgets a draft once it is sent or cleared", () => {
    const store = useAgentChatStore.getState();
    store.setInputDraft("t1", "send me");
    store.setInputDraft("t2", "keep me");
    flushComposerDraftBackupForTests();

    store.appendUserMessage("t1", "send me", "nonce-1");
    store.setInputDraft("t2", "");
    flushComposerDraftBackupForTests();
    expect(saved()).toBeNull();
  });

  it("seeds a new page's thread with the text the old page left", () => {
    window.sessionStorage.setItem(KEY, JSON.stringify({ t1: "typed before the reload" }));
    resetComposerDraftBackupForTests();

    useAgentChatStore.getState().ensureThread("t1");
    expect(useAgentChatStore.getState().threads.t1.inputDraft).toBe("typed before the reload");
    useAgentChatStore.getState().ensureThread("t2");
    expect(useAgentChatStore.getState().threads.t2.inputDraft).toBe("");
  });

  it("does not bring back a restored draft the user has since cleared", () => {
    window.sessionStorage.setItem(KEY, JSON.stringify({ t1: "old" }));
    resetComposerDraftBackupForTests();
    useAgentChatStore.getState().ensureThread("t1");
    useAgentChatStore.getState().setInputDraft("t1", "");
    flushComposerDraftBackupForTests();

    expect(restoredComposerDraft("t1")).toBe("");
    useAgentChatStore.setState({ threads: {} });
    useAgentChatStore.getState().ensureThread("t1");
    expect(useAgentChatStore.getState().threads.t1.inputDraft).toBe("");
    expect(saved()).toBeNull();
  });

  it("ignores unreadable storage", () => {
    window.sessionStorage.setItem(KEY, "{not json");
    resetComposerDraftBackupForTests();
    expect(restoredComposerDraft("t1")).toBe("");

    window.sessionStorage.setItem(KEY, JSON.stringify({ t1: 42, t2: "ok" }));
    resetComposerDraftBackupForTests();
    expect(restoredComposerDraft("t1")).toBe("");
    expect(restoredComposerDraft("t2")).toBe("ok");
  });
});
