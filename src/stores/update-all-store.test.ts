import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

import type { ProviderUpdateReport } from "@/stores/provider-update-store";
import {
  LOCAL,
  buildPlan,
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

describe("runPlan", () => {
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
