import { beforeEach, describe, expect, it, vi } from "vitest";
import { updateTargetKey, useProviderUpdates, type ProviderUpdateReport } from "./provider-update-store";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));
const target = { provider: "codex" as const };
const report: ProviderUpdateReport = { provider: "codex", installed_version: "1.0.0", latest_version: "1.1.0", available: true, manager: "mise", can_update: true, message: null };
beforeEach(() => { useProviderUpdates.setState({ slots: {} }); invoke.mockReset(); });
describe("provider updates", () => {
  it("coalesces concurrent checks and caches the answer", async () => {
    let resolve!: (value: ProviderUpdateReport) => void;
    invoke.mockReturnValue(new Promise(r => { resolve = r; }));
    const first = useProviderUpdates.getState().check(target);
    await useProviderUpdates.getState().check(target);
    expect(invoke).toHaveBeenCalledTimes(1);
    resolve(report); await first;
    await useProviderUpdates.getState().check(target);
    expect(invoke).toHaveBeenCalledTimes(1);
  });
  it("keeps failed updates retryable and requires backend verification for success", async () => {
    invoke.mockResolvedValueOnce(report);
    await useProviderUpdates.getState().check(target);
    invoke.mockRejectedValueOnce(new Error("Version did not change"));
    await useProviderUpdates.getState().update(target);
    expect(useProviderUpdates.getState().slots[updateTargetKey(target)]).toMatchObject({ updating: false, error: "Error: Version did not change" });
    invoke.mockResolvedValueOnce({ ...report, available: false, installed_version: "1.1.0" });
    await useProviderUpdates.getState().update(target);
    expect(useProviderUpdates.getState().slots[updateTargetKey(target)]).toMatchObject({ updated: true, error: undefined });
  });
  it("does not run the same installer twice", async () => {
    let resolve!: (value: ProviderUpdateReport) => void;
    invoke.mockReturnValue(new Promise(r => { resolve = r; }));
    const first = useProviderUpdates.getState().update(target);
    await useProviderUpdates.getState().update(target);
    expect(invoke).toHaveBeenCalledTimes(1);
    resolve({ ...report, available: false }); await first;
  });
  it("scopes Hermes checks to the selected installation", async () => {
    invoke.mockResolvedValue(report);
    await useProviderUpdates.getState().check({ provider: "hermes", installation: "/one/hermes" });
    await useProviderUpdates.getState().check({ provider: "hermes", installation: "/two/hermes" });
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(invoke).toHaveBeenLastCalledWith("agent_chat_provider_update_check", { provider: "hermes", installation: "/two/hermes" });
  });
  it("does not turn an offline check into an available update", async () => {
    invoke.mockRejectedValue(new Error("Offline"));
    await useProviderUpdates.getState().check(target);
    expect(useProviderUpdates.getState().slots[updateTargetKey(target)].report).toBeUndefined();
  });
});
