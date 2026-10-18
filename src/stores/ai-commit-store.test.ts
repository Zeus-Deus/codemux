import { beforeEach, describe, expect, it, vi } from "vitest";
import { generateAiCommitMessage } from "@/tauri/commands";
import { useAiCommitStore } from "./ai-commit-store";

vi.mock("@/tauri/commands", () => ({ generateAiCommitMessage: vi.fn() }));

function pendingMessage() {
  let resolve!: (message: string) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<string>((res, rej) => { resolve = res; reject = rej; });
  vi.mocked(generateAiCommitMessage).mockReturnValueOnce(promise);
  return { resolve, reject };
}

async function flush() {
  await Promise.resolve();
  await Promise.resolve();
}

beforeEach(() => {
  useAiCommitStore.setState({ generations: {} });
  vi.mocked(generateAiCommitMessage).mockReset();
});

describe("commit message request ownership", () => {
  it("keeps the newest message when requests finish out of order", async () => {
    const old = pendingMessage();
    const current = pendingMessage();
    const store = useAiCommitStore.getState();
    store.requestGeneration("workspace", "/repo", null, null);
    store.requestGeneration("workspace", "/repo", null, null);
    current.resolve("Current changes");
    await flush();
    old.resolve("Outdated changes");
    await flush();

    expect(store.consumeMessage("workspace")).toBe("Current changes");
    expect(store.getGeneration("workspace")).toBeUndefined();
  });

  it("ignores errors from a superseded request", async () => {
    const old = pendingMessage();
    const current = pendingMessage();
    const store = useAiCommitStore.getState();
    store.requestGeneration("workspace", "/repo", null, null);
    store.requestGeneration("workspace", "/repo", null, null);
    old.reject(new Error("Old failure"));
    await flush();
    expect(store.getGeneration("workspace")?.status).toBe("generating");
    current.resolve("Current message");
    await flush();
    expect(store.consumeMessage("workspace")).toBe("Current message");
  });

  it.each(["resolve", "reject"] as const)("does not recreate a cleared generation after %s", async (finish) => {
    const request = pendingMessage();
    const store = useAiCommitStore.getState();
    store.requestGeneration("workspace", "/repo", null, null);
    store.clearGeneration("workspace");
    if (finish === "resolve") request.resolve("Dismissed message");
    else request.reject(new Error("Dismissed error"));
    await flush();
    expect(store.getGeneration("workspace")).toBeUndefined();
  });

  it("keeps concurrent workspaces independent", async () => {
    const first = pendingMessage();
    const second = pendingMessage();
    const store = useAiCommitStore.getState();
    store.requestGeneration("one", "/one", null, null);
    store.requestGeneration("two", "/two", null, null);
    second.resolve("Second message");
    first.resolve("First message");
    await flush();
    expect(store.consumeMessage("one")).toBe("First message");
    expect(store.consumeMessage("two")).toBe("Second message");
  });
});
