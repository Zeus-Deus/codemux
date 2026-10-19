import { beforeEach, describe, expect, it, vi } from "vitest";
import { Owner } from "./lifetime";
const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("./mcp", () => ({ pluginPageEpoch: async () => 42 }));
import { createNativeHost } from "./host-native";
const flush = async () => {
  for (let i = 0; i < 8; i++) await Promise.resolve();
};
beforeEach(() => {
  mocks.invoke.mockReset();
  mocks.invoke.mockResolvedValue(undefined);
});
describe("plugin native host", () => {
  it("retries an acquire after a transient failure", async () => {
    mocks.invoke.mockRejectedValueOnce(new Error("transient acquire failure"));
    const owner = new Owner("demo");
    const host = createNativeHost(() => owner);
    await expect(host.process.which("unity")).rejects.toThrow("transient");
    await expect(host.process.which("unity")).resolves.toBeUndefined();
    expect(mocks.invoke.mock.calls.filter(([name]) => name === "plugins_host_acquire")).toHaveLength(2);
    owner.end();
  });
  it("acquires once with the document epoch and releases once on unload", async () => {
    const owner = new Owner("demo");
    const host = createNativeHost(() => owner);
    await host.process.which("unity");
    await host.fs.exists("/project/Assets");
    expect(
      mocks.invoke.mock.calls.filter(
        ([name]) => name === "plugins_host_acquire",
      ),
    ).toHaveLength(1);
    expect(mocks.invoke).toHaveBeenCalledWith(
      "plugins_host_acquire",
      expect.objectContaining({ pageEpoch: 42, pluginId: "demo" }),
    );
    owner.end();
    owner.end();
    await flush();
    expect(
      mocks.invoke.mock.calls.filter(
        ([name]) => name === "plugins_host_release",
      ),
    ).toHaveLength(1);
    await expect(host.process.which("unity")).rejects.toThrow(
      "no longer active",
    );
  });
  it("releases a native owner when unload races the acquire response", async () => {
    let finish!: () => void;
    mocks.invoke.mockImplementation((name: string) =>
      name === "plugins_host_acquire"
        ? new Promise<void>((resolve) => {
            finish = resolve;
          })
        : Promise.resolve(),
    );
    const owner = new Owner("demo");
    const host = createNativeHost(() => owner);
    const pending = host.process.run({ command: "unity" });
    const rejected = expect(pending).rejects.toThrow("disposed");
    await flush();
    owner.end();
    finish();
    await rejected;
    expect(mocks.invoke.mock.calls.map(([name]) => name)).toEqual([
      "plugins_host_acquire",
      "plugins_host_release",
    ]);
  });
  it("passes argv, environment removal, bounds, local workspace and data names intact", async () => {
    const owner = new Owner("demo");
    const host = createNativeHost(() => owner);
    const input = {
      command: "unity",
      args: ["status", "--json"],
      cwd: "/project",
      env: { HOME: null, PATH: "/bin" },
      workspaceId: "local",
      maxOutputBytes: 100,
      timeoutMs: 1000,
    };
    const info = {
      platform: "linux",
      arch: "x86_64",
      homeDir: "/home/user",
      configDir: "/home/user/.config",
      dataDir: "/home/user/.local/share",
      localDataDir: "/home/user/.local/share",
    };
    mocks.invoke.mockImplementation((command: string) =>
      Promise.resolve(command === "plugins_os_info" ? info : undefined),
    );
    expect(await host.os.info()).toEqual(info);
    await host.process.run(input);
    expect(mocks.invoke).toHaveBeenLastCalledWith(
      "plugins_process_run",
      expect.objectContaining({ input }),
    );
    await host.fs.readText("/project/a", {
      maxBytes: 123,
      workspaceId: "local",
    });
    expect(mocks.invoke).toHaveBeenLastCalledWith(
      "plugins_fs",
      expect.objectContaining({
        workspaceId: "local",
        operation: { kind: "readText", path: "/project/a", maxBytes: 123 },
      }),
    );
    await host.fs.writeDataFile("policy.json", "{}");
    expect(mocks.invoke).toHaveBeenLastCalledWith(
      "plugins_fs",
      expect.objectContaining({
        operation: {
          kind: "writeDataFile",
          name: "policy.json",
          contents: "{}",
        },
      }),
    );
    owner.end();
  });
  it("refuses ambiguous global host calls after an await", async () => {
    await expect(createNativeHost(() => null).fs.dataDir()).rejects.toThrow(
      "use ctx.host",
    );
    expect(mocks.invoke).not.toHaveBeenCalled();
  });
});
