import { beforeEach, describe, expect, it, vi } from "vitest";

const writes: { session: string; data: string; resolve: () => void }[] = [];
vi.mock("@/tauri/commands", () => ({
  writeToPty: vi.fn(
    (session: string, data: string) =>
      new Promise<void>((resolve) => writes.push({ session, data, resolve })),
  ),
}));

import { writePtyInput } from "./pty-input";

async function settle() {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

describe("writePtyInput", () => {
  beforeEach(() => {
    writes.length = 0;
  });

  it("keeps one write in flight and coalesces later input in order", async () => {
    writePtyInput("s1", "echo C");
    writePtyInput("s1", "ORE");
    writePtyInput("s1", "_4");
    expect(writes.map((w) => w.data)).toEqual(["echo C"]);
    writes[0].resolve();
    await settle();
    expect(writes.map((w) => w.data)).toEqual(["echo C", "ORE_4"]);
    writePtyInput("s1", "\r");
    writes[1].resolve();
    await settle();
    expect(writes.map((w) => w.data)).toEqual(["echo C", "ORE_4", "\r"]);
    writes[2].resolve();
    await settle();
    // Idle again: the next input is sent at once.
    writePtyInput("s1", "x");
    expect(writes.map((w) => w.data)).toEqual(["echo C", "ORE_4", "\r", "x"]);
    writes[3].resolve();
    await settle();
  });

  it("orders each session independently and continues after a failed write", async () => {
    const commands = await import("@/tauri/commands");
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.mocked(commands.writeToPty).mockImplementationOnce(() =>
      Promise.reject(new Error("closed")),
    );
    writePtyInput("a", "1");
    writePtyInput("b", "2");
    writePtyInput("a", "3");
    await settle();
    // "a"'s first write failed; its queued input still goes out, in order.
    expect(writes.map((w) => `${w.session}:${w.data}`)).toEqual(["b:2", "a:3"]);
    expect(error).toHaveBeenCalledOnce();
    for (const w of writes) w.resolve();
    await settle();
    error.mockRestore();
  });
});
