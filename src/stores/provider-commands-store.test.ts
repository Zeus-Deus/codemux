import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@/tauri/commands", () => ({ listChatSlashCommands: vi.fn() }));

import { listChatSlashCommands } from "@/tauri/commands";
import { commandsKey, useProviderCommandsStore } from "./provider-commands-store";

const list = vi.mocked(listChatSlashCommands);
const commands = [{ name: "compact", description: "Free context", argumentHint: "" }];

describe("provider command discovery", () => {
  beforeEach(() => {
    useProviderCommandsStore.getState().invalidate();
    list.mockReset().mockResolvedValue(commands);
  });
  afterEach(() => vi.useRealTimers());

  it("discovers Home commands without sharing their catalogue with a project", async () => {
    await useProviderCommandsStore.getState().loadCommands("claude", null);
    expect(list).toHaveBeenCalledWith("claude", null, false);
    await useProviderCommandsStore.getState().loadCommands("claude", "/repo");
    expect(Object.keys(useProviderCommandsStore.getState().entries)).toHaveLength(2);
  });

  it("isolates every ACP conversation's live command catalogue", async () => {
    for (const provider of ["cursor", "grok", "hermes"] as const) {
      list.mockResolvedValueOnce([{ name: "session-only", description: "", argumentHint: "" }]);
      await useProviderCommandsStore.getState().loadCommands(provider, "/repo", true, "a");
      await useProviderCommandsStore.getState().loadCommands(provider, "/repo", true, "b");
      expect(list).toHaveBeenLastCalledWith(provider, "/repo", true, "b");
      expect(useProviderCommandsStore.getState().entries[commandsKey(provider, "/repo", "a")].commands[0].name).toBe("session-only");
      expect(useProviderCommandsStore.getState().entries[commandsKey(provider, "/repo", "b")].commands[0].name).toBe("compact");
    }
  });

  it("caches briefly, then discovers installed or edited commands", async () => {
    vi.useFakeTimers();
    const store = useProviderCommandsStore.getState();
    await store.loadCommands("claude", "/repo");
    await store.loadCommands("claude", "/repo");
    expect(list).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(60_001);
    await store.loadCommands("claude", "/repo");
    expect(list).toHaveBeenCalledTimes(2);
  });

  it("forces a backend refresh and isolates provider/project entries", async () => {
    const store = useProviderCommandsStore.getState();
    await store.loadCommands("claude", "/repo");
    await store.loadCommands("claude", "/repo", true);
    expect(list).toHaveBeenLastCalledWith("claude", "/repo", true);
    await store.loadCommands("codex", "/other");
    expect(Object.keys(useProviderCommandsStore.getState().entries)).toHaveLength(2);
  });

  it("does not restore a catalogue invalidated during discovery", async () => {
    let finish!: (value: typeof commands) => void;
    list.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    const pending = useProviderCommandsStore.getState().loadCommands("claude", "/repo");
    useProviderCommandsStore.getState().invalidate();
    finish(commands);
    await pending;
    expect(useProviderCommandsStore.getState().entries).toEqual({});
  });

  it("retains the last catalogue on a refresh failure and permits retry", async () => {
    const store = useProviderCommandsStore.getState();
    await store.loadCommands("claude", "/repo");
    list.mockRejectedValueOnce(new Error("CLI unavailable"));
    await store.loadCommands("claude", "/repo", true);
    const entry = useProviderCommandsStore.getState().entries[commandsKey("claude", "/repo")];
    expect(entry.commands).toEqual(commands);
    expect(entry.error).toBe("CLI unavailable");
    await store.loadCommands("claude", "/repo", true);
    expect(useProviderCommandsStore.getState().entries[commandsKey("claude", "/repo")].error).toBeNull();
  });
  it("isolates Hermes profiles by their conversation instead of merging their cwd", async () => {
    const store = useProviderCommandsStore.getState();
    list.mockResolvedValueOnce([{ name: "learn", description: "Learn", argumentHint: "" }]);
    await store.loadCommands("hermes", "/repo", true, "thread-a");
    await store.loadCommands("hermes", "/repo", true, "thread-b");
    expect(list).toHaveBeenLastCalledWith("hermes", "/repo", true, "thread-b");
    expect(useProviderCommandsStore.getState().entries[commandsKey("hermes", "/repo", "thread-a")].commands[0].name).toBe("learn");
    expect(useProviderCommandsStore.getState().entries[commandsKey("hermes", "/repo", "thread-b")].commands[0].name).toBe("compact");
  });

});
