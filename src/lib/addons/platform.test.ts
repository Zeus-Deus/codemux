import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
vi.mock("./bridge", () => ({
  addonInvoke: vi.fn(),
  addonInventory: vi.fn(),
  subscribeAddons: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("sonner", () => ({ toast: { info: vi.fn(), error: vi.fn() } }));
import { addonInvoke, addonInventory } from "./bridge";
import { applyAddonEffect, refreshAddons } from "./platform";
import { registerAddonComposer } from "./composer-registry";
import {
  beginAddonRevocation,
  clearAddonContext,
  useAddonsStore,
} from "@/stores/addons-store";
import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import type { AppStateSnapshot } from "@/tauri/types";
import type { AddonEvent, AddonInstallation, AddonInventory } from "./types";
const installation = {
  installationId: "installation",
  manifest: { id: "test.plugin", name: "Fixture" },
  desiredEnabled: true,
  status: "enabled-running",
} as AddonInstallation;
const event: Extract<AddonEvent, { type: "effect" }> = {
  type: "effect",
  requestId: "request",
  generation: "generation",
  pluginId: "test.plugin",
  operation: "composer.appendText",
  params: { workspaceId: "workspace", composerId: "composer", text: "brief" },
};
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}
let unregister: () => void;
const append = vi.fn(() => 1);
beforeEach(() => {
  vi.resetAllMocks();
  useAppStore.setState({
    appState: {
      active_workspace_id: "workspace",
      workspaces: [{ workspace_id: "workspace" }],
    } as AppStateSnapshot,
    pendingActiveWorkspaceId: null,
  });
  useFeatureFlags.setState({ enableAgentChat: true });
  useAddonsStore.setState({
    installed: [installation],
    paused: false,
    ready: true,
    failures: {},
    revoking: {},
    error: null,
  });
  unregister = registerAddonComposer("composer", {
    workspaceId: "workspace",
    append,
  });
  vi.mocked(addonInvoke).mockResolvedValue(null);
});
afterEach(() => unregister());
describe("delayed native effect completion", () => {
  it.each(["disable", "remove", "pause", "workspace", "thread", "failure"])(
    "does not insert after %s while a claim is delayed",
    async (action) => {
      const claim = deferred<unknown>();
      vi.mocked(addonInvoke).mockImplementation((command) =>
        command === "addon_effect_claim"
          ? claim.promise
          : Promise.resolve(null),
      );
      const pending = applyAddonEffect(event);
      let release: (() => void) | undefined;
      if (action === "disable" || action === "remove")
        release = beginAddonRevocation("test.plugin");
      if (action === "pause") release = beginAddonRevocation("*");
      if (action === "workspace") clearAddonContext();
      if (action === "thread") unregister();
      if (action === "failure")
        useAddonsStore.setState({ failures: { generation: "Child stopped" } });
      claim.resolve(null);
      await pending;
      expect(append).not.toHaveBeenCalled();
      expect(addonInvoke).toHaveBeenLastCalledWith(
        "addon_effect_result",
        expect.objectContaining({ error: expect.any(Object) }),
      );
      release?.();
    },
  );
  it("applies an authorized current effect once and reports its result", async () => {
    await applyAddonEffect(event);
    expect(append).toHaveBeenCalledExactlyOnceWith("brief");
    expect(addonInvoke).toHaveBeenLastCalledWith(
      "addon_effect_result",
      expect.objectContaining({ error: null, value: 1 }),
    );
  });
  it("does not let an old inventory response restore an enabled installation", async () => {
    const old = deferred<AddonInventory>(),
      current = deferred<AddonInventory>();
    vi.mocked(addonInventory)
      .mockReturnValueOnce(old.promise)
      .mockReturnValueOnce(current.promise);
    const first = refreshAddons(),
      second = refreshAddons();
    current.resolve({ installed: [], paused: true, error: null });
    await second;
    old.resolve({ installed: [installation], paused: false, error: null });
    await first;
    expect(useAddonsStore.getState().paused).toBe(true);
    expect(useAddonsStore.getState().installed).toEqual([]);
  });
});
