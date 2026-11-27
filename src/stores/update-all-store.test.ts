import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

import type { ProviderUpdateReport } from "@/stores/provider-update-store";
import {
  LOCAL,
  buildPlan,
  helperNeedsRestart,
  hostKey,
  isOlderVersion,
  itemKey,
  useUpdateAll,
  type MachineState,
} from "./update-all-store";

const report = (provider: ProviderUpdateReport["provider"], patch: Partial<ProviderUpdateReport> = {}): ProviderUpdateReport => ({
  provider,
  installed_version: "1.0.0",
  latest_version: "1.1.0",
  available: true,
  manager: "npm",
  can_update: true,
  message: null,
  ...patch,
});

beforeEach(() => {
  invoke.mockReset();
  useUpdateAll.setState({ machines: {}, items: {}, running: false, summary: null });
});

describe("isOlderVersion", () => {
  it("compares numerically, not as text", () => {
    expect(isOlderVersion("0.9.0", "0.10.0")).toBe(true);
    expect(isOlderVersion("0.24.0", "0.24.0")).toBe(false);
    expect(isOlderVersion("0.25.0", "0.24.0")).toBe(false);
    expect(isOlderVersion("v1.2", "1.2.1")).toBe(true);
  });
});

describe("buildPlan", () => {
  it("updates outdated helpers and updatable providers, and names everything it skips", () => {
    const machines: Record<string, MachineState> = {
      [LOCAL]: {
        loading: false,
        providers: [
          report("claude", { manager: "Omarchy · mise" }),
          report("codex", { available: false }),
          report("cursor", { can_update: false, message: "Run `omarchy update`" }),
        ],
      },
      [hostKey(2)]: { loading: false, providers: [report("opencode")], helperVersion: "0.23.0", appVersion: "0.24.0" },
      [hostKey(3)]: { loading: false, providers: [], helperVersion: null, error: "couldn't connect" },
      [hostKey(4)]: { loading: false, providers: [], helperVersion: "0.25.0", appVersion: "0.24.0" },
    };
    const plan = buildPlan(machines, { [hostKey(2)]: "workstation", [hostKey(3)]: "nas" });
    expect(plan.items.map((i) => `${i.machine}/${i.item}`)).toEqual([
      "local/claude",
      "host:2/helper",
      "host:2/opencode",
    ]);
    expect(plan.skipped).toEqual([
      { machine: LOCAL, label: "Cursor", reason: "Run `omarchy update`" },
      { machine: hostKey(3), label: "nas", reason: "couldn't connect" },
    ]);
  });
});

describe("refreshDevice", () => {
  it.each(["reported error", "rejected check"])("skips cached device updates after a %s", async (failure) => {
    const key = hostKey(2);
    const previous = {
      loading: false,
      checkedAt: 123,
      providers: [report("codex")],
      helperVersion: "0.23.0",
      appVersion: "0.24.0",
    };
    useUpdateAll.setState({ machines: { [key]: previous } });
    if (failure === "rejected check") {
      invoke.mockRejectedValue(new Error("SSH timed out"));
    } else {
      invoke.mockResolvedValue({
        host_id: 2,
        helper_version: null,
        running_version: null,
        serve_alive: null,
        app_version: "0.24.0",
        providers: null,
        error: "SSH timed out",
      });
    }

    await useUpdateAll.getState().refreshDevice(2, true);

    const machines = useUpdateAll.getState().machines as Record<string, MachineState>;
    expect(machines[key].providers).toEqual(previous.providers);
    expect(machines[key].helperVersion).toBe(previous.helperVersion);
    expect(machines[key].checkedAt).toBe(previous.checkedAt);
    expect(machines[key].latestCheck).toEqual({ helperVersion: null, providerInventoryFresh: false });
    expect(buildPlan(machines, { [key]: "workstation" })).toEqual({
      items: [],
      skipped: [{ machine: key, label: "workstation", reason: failure === "rejected check" ? "Error: SSH timed out" : "SSH timed out" }],
    });
  });

  it("upgrades a reachable old helper before discovering its agents", async () => {
    const key = hostKey(2);
    const previous = {
      loading: false,
      checkedAt: 123,
      providers: [report("codex")],
      helperVersion: "0.22.0",
      appVersion: "0.24.0",
    };
    useUpdateAll.setState({ machines: { [key]: previous } });
    invoke.mockResolvedValueOnce({
      host_id: 2,
      helper_version: "0.23.0",
      running_version: null,
      serve_alive: null,
      app_version: "0.24.0",
      providers: null,
      error: "Update Codemux on this device to manage its agents",
    });

    await useUpdateAll.getState().refreshDevice(2, true);

    const state = useUpdateAll.getState().machines[key]!;
    expect(state.providers).toEqual(previous.providers);
    expect(state.helperVersion).toBe("0.23.0");
    expect(state.checkedAt).toBe(previous.checkedAt);
    expect(state.latestCheck).toEqual({ helperVersion: "0.23.0", providerInventoryFresh: false });
    const plan = buildPlan(useUpdateAll.getState().machines as Record<string, MachineState>, { [key]: "workstation" });
    expect(plan).toEqual({ items: [{ machine: key, item: "helper", label: "Codemux" }], skipped: [] });

    invoke.mockReset();
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "device_helper_update") {
        return { host_id: 2, status: "upgraded", from: "0.23.0", to: "0.24.0", message: null };
      }
      if (cmd === "provider_updates_device") {
        return { host_id: 2, helper_version: "0.24.0", running_version: null, serve_alive: null, app_version: "0.24.0", providers: [report("grok")], error: null };
      }
      if (cmd === "provider_update_device") return report("grok", { installed_version: "1.1.0", available: false });
      throw new Error(`unexpected ${cmd}`);
    });

    expect(await useUpdateAll.getState().runPlan(plan)).toMatchObject({ updated: 2, failed: [] });
    expect(invoke.mock.calls.map(([cmd]) => cmd)).toEqual([
      "device_helper_update", "provider_updates_device", "provider_update_device", "provider_updates_device",
    ]);
  });

  it("ignores a pre-install check that finishes after a helper update", async () => {
    useUpdateAll.setState({
      machines: {
        [hostKey(2)]: { loading: false, providers: [], helperVersion: "0.23.0", runningVersion: null, appVersion: "0.24.0" },
      },
    });
    let finishUpdate!: () => void;
    let finishOldCheck!: () => void;
    const updateGate = new Promise<void>((resolve) => { finishUpdate = resolve; });
    const oldCheckGate = new Promise<void>((resolve) => { finishOldCheck = resolve; });
    let checks = 0;
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "device_helper_update") {
        await updateGate;
        return { host_id: 2, status: "upgraded", from: "0.23.0", to: "0.24.0", message: null };
      }
      if (cmd === "provider_updates_device") {
        checks++;
        if (checks === 1) {
          await oldCheckGate;
          return { host_id: 2, helper_version: "0.23.0", running_version: null, serve_alive: null, app_version: "0.24.0", providers: [], error: null };
        }
        return { host_id: 2, helper_version: "0.24.0", running_version: "0.23.0", serve_alive: true, app_version: "0.24.0", providers: [report("codex")], error: null };
      }
      throw new Error(`unexpected ${cmd}`);
    });

    const update = useUpdateAll.getState().updateOne(hostKey(2), "helper");
    const oldCheck = useUpdateAll.getState().refreshDevice(2, true);
    finishUpdate();
    expect(await update).toBe(true);
    finishOldCheck();
    await oldCheck;

    const state = useUpdateAll.getState().machines[hostKey(2)];
    expect(checks).toBe(2);
    expect(state?.helperVersion).toBe("0.24.0");
    expect(state?.providers.map((provider) => provider.provider)).toEqual(["codex"]);
    expect(state?.loading).toBe(false);
    expect(helperNeedsRestart(state)).toBe(true);
  });
});

describe("updateOne", () => {
  it.each([false, true])("refreshes helper and daemon status after a direct agent update (installer fails: %s)", async (fails) => {
    useUpdateAll.setState({
      machines: {
        [hostKey(2)]: { loading: false, providers: [report("codex")], helperVersion: "0.23.0", runningVersion: null, appVersion: "0.24.0" },
      },
    });
    let installedVersion = "0.23.0";
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "provider_update_device") {
        installedVersion = "0.24.0";
        if (fails) throw new Error("npm failed (exit 1)");
        return report("codex", { installed_version: "1.1.0", available: false });
      }
      if (cmd === "provider_updates_device") {
        return {
          host_id: 2,
          helper_version: installedVersion,
          running_version: "0.23.0",
          serve_alive: true,
          app_version: "0.24.0",
          providers: [report("codex", { available: fails })],
          error: null,
        };
      }
      throw new Error(`unexpected ${cmd}`);
    });

    expect(await useUpdateAll.getState().updateOne(hostKey(2), "codex")).toBe(!fails);

    const state = useUpdateAll.getState();
    expect(invoke.mock.calls.map(([cmd]) => cmd)).toEqual(["provider_update_device", "provider_updates_device"]);
    expect(invoke).toHaveBeenCalledWith("provider_updates_device", { hostId: 2, force: true });
    expect(state.machines[hostKey(2)]?.helperVersion).toBe("0.24.0");
    expect(state.machines[hostKey(2)]?.runningVersion).toBe("0.23.0");
    expect(helperNeedsRestart(state.machines[hostKey(2)])).toBe(true);
    expect(state.items[itemKey(hostKey(2), "codex")]).toEqual(
      fails ? { phase: "failed", message: "npm failed (exit 1)" } : { phase: "updated" },
    );
  });
});

describe("runPlan", () => {
  it("does not use cached agents when the check after a helper upgrade fails", async () => {
    const key = hostKey(2);
    useUpdateAll.setState({
      machines: { [key]: { loading: false, providers: [report("codex")], helperVersion: "0.23.0", appVersion: "0.24.0" } },
    });
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "device_helper_update") {
        return { host_id: 2, status: "upgraded", from: "0.23.0", to: "0.24.0", message: null };
      }
      if (cmd === "provider_updates_device") throw new Error("SSH timed out");
      throw new Error(`unexpected ${cmd}`);
    });

    const plan = buildPlan(useUpdateAll.getState().machines as Record<string, MachineState>, {});
    const summary = await useUpdateAll.getState().runPlan(plan);

    expect(invoke.mock.calls.map(([cmd]) => cmd)).toEqual(["device_helper_update", "provider_updates_device"]);
    expect(summary).toMatchObject({
      updated: 1,
      failed: [{ machine: key, label: "Agent discovery", reason: "Error: SSH timed out" }],
    });
    expect(useUpdateAll.getState().machines[key]?.providers).toEqual([report("codex")]);
    expect(useUpdateAll.getState().items[itemKey(key, "helper")]?.phase).toBe("updated");
    expect(useUpdateAll.getState().items[itemKey(key, "codex")]).toEqual({ phase: "failed", message: "Error: SSH timed out" });
  });

  it("reports failed agent discovery after a helper-only bulk upgrade", async () => {
    const key = hostKey(2);
    useUpdateAll.setState({
      machines: {
        [key]: {
          loading: false,
          providers: [],
          helperVersion: "0.23.0",
          appVersion: "0.24.0",
          error: "Update Codemux on this device to manage its agents",
          latestCheck: { helperVersion: "0.23.0", providerInventoryFresh: false },
        },
      },
    });
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "device_helper_update") {
        return { host_id: 2, status: "upgraded", from: "0.23.0", to: "0.24.0", message: null };
      }
      if (cmd === "provider_updates_device") throw new Error("SSH timed out");
      throw new Error(`unexpected ${cmd}`);
    });

    const plan = buildPlan(useUpdateAll.getState().machines as Record<string, MachineState>, {});
    expect(plan.items).toEqual([{ machine: key, item: "helper", label: "Codemux" }]);
    const summary = await useUpdateAll.getState().runPlan(plan);

    expect(invoke.mock.calls.map(([cmd]) => cmd)).toEqual(["device_helper_update", "provider_updates_device"]);
    expect(summary).toMatchObject({
      updated: 1,
      failed: [{ machine: key, label: "Agent discovery", reason: "Error: SSH timed out" }],
    });
    expect(useUpdateAll.getState().items[itemKey(key, "helper")]?.phase).toBe("updated");
  });

  it("updates a device's helper before its providers and reports partial failure", async () => {
    useUpdateAll.setState({
      machines: {
        [LOCAL]: { loading: false, providers: [report("claude")] },
        [hostKey(2)]: { loading: false, providers: [report("codex")], helperVersion: "0.23.0", appVersion: "0.24.0" },
      },
    });
    const calls: string[] = [];
    invoke.mockImplementation(async (cmd: string, args: Record<string, unknown>) => {
      calls.push(`${cmd}:${String(args.provider ?? args.hostId)}`);
      if (cmd === "device_helper_update") return { host_id: 2, status: "upgraded", from: "0.23.0", to: "0.24.0", message: null };
      // The re-check after the helper update: the new helper also finds Grok.
      if (cmd === "provider_updates_device") {
        return { host_id: 2, helper_version: "0.24.0", app_version: "0.24.0", providers: [report("codex"), report("grok")], error: null };
      }
      if (cmd === "provider_update_device") throw "npm exited with exit status: 1";
      return report(args.provider as "claude", { installed_version: "1.1.0", available: false });
    });
    const plan = buildPlan(useUpdateAll.getState().machines as Record<string, MachineState>, {});
    const summary = await useUpdateAll.getState().runPlan(plan);

    expect(calls.indexOf("device_helper_update:2")).toBeLessThan(calls.indexOf("provider_updates_device:2"));
    expect(calls.indexOf("provider_updates_device:2")).toBeLessThan(calls.indexOf("provider_update_device:codex"));
    expect(calls).toContain("provider_update_device:grok");
    expect(summary.updated).toBe(2);
    expect(summary.failed).toEqual([
      { machine: hostKey(2), label: "Codex", reason: "npm exited with exit status: 1" },
      { machine: hostKey(2), label: "Grok", reason: "npm exited with exit status: 1" },
    ]);
    const state = useUpdateAll.getState();
    expect(state.running).toBe(false);
    expect(state.items[itemKey(LOCAL, "claude")].phase).toBe("updated");
    expect(state.items[itemKey(hostKey(2), "codex")].phase).toBe("failed");
    expect(state.machines[hostKey(2)]?.helperVersion).toBe("0.24.0");
    expect(state.machines[LOCAL]?.providers[0].available).toBe(false);
  });

  it("does not run a device's provider updates when its helper update fails", async () => {
    useUpdateAll.setState({
      machines: { [hostKey(6)]: { loading: false, providers: [report("codex")], helperVersion: "0.1.0", appVersion: "0.24.0" } },
    });
    invoke.mockRejectedValue("upload: connection reset");
    const summary = await useUpdateAll.getState().runPlan({
      items: [
        { machine: hostKey(6), item: "helper", label: "Codemux" },
        { machine: hostKey(6), item: "codex", label: "Codex" },
      ],
      skipped: [],
    });
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(summary.failed).toEqual([{ machine: hostKey(6), label: "Codemux", reason: "upload: connection reset" }]);
    expect(useUpdateAll.getState().items[itemKey(hostKey(6), "codex")]).toBeUndefined();
  });

  it("treats a device it could not reach as a failure, not an update", async () => {
    useUpdateAll.setState({
      machines: { [hostKey(5)]: { loading: false, providers: [], helperVersion: "0.1.0", appVersion: "0.24.0" } },
    });
    invoke.mockResolvedValue({ host_id: 5, status: "skipped", from: null, to: null, message: "couldn't reach it: timed out" });
    const summary = await useUpdateAll.getState().runPlan({
      items: [{ machine: hostKey(5), item: "helper", label: "Codemux" }],
      skipped: [],
    });
    expect(summary.updated).toBe(0);
    expect(summary.failed[0].reason).toContain("timed out");
  });
});
