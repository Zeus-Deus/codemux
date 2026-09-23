import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";
vi.mock("./bridge", () => ({
  addonInvoke: vi.fn(),
  addonInventory: vi.fn(),
  subscribeAddons: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("sonner", () => ({ toast: { info: vi.fn(), error: vi.fn() } }));
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { addonInvoke, addonInventory, subscribeAddons } from "./bridge";
import {
  activeAddonWorkspace,
  applyAddonEffect,
  refreshAddons,
  useAddonPlatform,
} from "./platform";
import { registerAddonComposer } from "./composer-registry";
import { useAddonComposerAdapter } from "./use-addon-composer-adapter";
import {
  beginAddonRevocation,
  clearAddonContext,
  useAddonsStore,
} from "@/stores/addons-store";
import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import type { AppStateSnapshot } from "@/tauri/types";
import type {
  AddonEvent,
  AddonInstallation,
  AddonInventory,
  AddonManifest,
} from "./types";
const installation = {
  installationId: "installation",
  manifest: {
    id: "test.plugin",
    name: "Fixture",
    permissions: ["composer.append", "external.open"],
    contributes: {
      commands: [],
      panels: [{ id: "brief", title: "Brief", icon: "file-text" }],
      composerActions: [],
      composerViews: [{ id: "issues", title: "Issues", icon: "github" }],
    },
  } as unknown as AddonManifest,
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
const effect = (
  operation: string,
  params: Record<string, unknown>,
): Extract<AddonEvent, { type: "effect" }> => ({
  ...event,
  operation,
  params: { workspaceId: "workspace", composerId: "composer", ...params },
});
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}
const lastResult = () => {
  const results = vi
    .mocked(addonInvoke)
    .mock.calls.filter(([command]) => command === "addon_effect_result");
  return results[results.length - 1]?.[1] as {
    value: unknown;
    error: { message: string } | null;
  };
};
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
    accessory: null,
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
    expect(toast.error).not.toHaveBeenCalled();
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
describe("rejected effects are explained to the user", () => {
  it.each([
    [
      "composer.appendText",
      { composerId: "closed-composer", text: "brief" },
      "Fixture couldn't add text to the draft",
      "This chat composer is no longer available",
    ],
    [
      "composerViews.open",
      { id: "someone-elses" },
      "Fixture couldn't open its composer view",
      "Contribution does not belong to this plugin",
    ],
    [
      "panels.open",
      { id: "undeclared" },
      "Fixture couldn't open its panel",
      "Contribution does not belong to this plugin",
    ],
    [
      "links.open",
      { url: "http://example.com" },
      "Fixture couldn't open the link",
      "Only HTTPS links can be opened",
    ],
    [
      "links.open",
      { url: "https://user:secret@example.com/" },
      "Fixture couldn't open the link",
      "Only HTTPS links can be opened",
    ],
  ])(
    "%s: attributed toast with the specific reason, and the plugin gets it too",
    async (operation, params, title, reason) => {
      await applyAddonEffect(effect(operation, params));
      expect(toast.error).toHaveBeenCalledExactlyOnceWith(title, {
        id: "addon-effect:test.plugin",
        description: reason,
      });
      expect(lastResult().error).toMatchObject({ message: reason });
      expect(append).not.toHaveBeenCalled();
      expect(openUrl).not.toHaveBeenCalled();
    },
  );
  it("says the workspace changed when a delayed effect outlives its project", async () => {
    const claim = deferred<unknown>();
    vi.mocked(addonInvoke).mockImplementation((command) =>
      command === "addon_effect_claim" ? claim.promise : Promise.resolve(null),
    );
    const pending = applyAddonEffect(event);
    clearAddonContext();
    claim.resolve(null);
    await pending;
    expect(toast.error).toHaveBeenCalledWith(
      "Fixture couldn't add text to the draft",
      expect.objectContaining({ description: "The active workspace changed" }),
    );
  });
  it("names pause-all, not a generic stop, when effects are fenced", async () => {
    useAddonsStore.setState({ paused: true });
    await applyAddonEffect(effect("panels.open", { id: "brief" }));
    expect(toast.error).toHaveBeenCalledWith(
      "Fixture couldn't open its panel",
      expect.objectContaining({ description: "Add-ons are paused" }),
    );
  });
  it("explains an expired claim instead of failing silently", async () => {
    vi.mocked(addonInvoke).mockImplementation((command) =>
      command === "addon_effect_claim"
        ? Promise.reject({
            message: "The add-on action expired",
            data: { code: "CONTEXT_STALE" },
          })
        : Promise.resolve(null),
    );
    await applyAddonEffect(effect("panels.open", { id: "brief" }));
    expect(toast.error).toHaveBeenCalledWith(
      "Fixture couldn't open its panel",
      expect.objectContaining({ description: "The add-on action expired" }),
    );
  });
  it("does not raise an error toast for a refused notification", async () => {
    useAddonsStore.setState({ paused: true });
    await applyAddonEffect(effect("ui.notify", { message: "hello" }));
    expect(toast.error).not.toHaveBeenCalled();
    expect(lastResult().error).not.toBeNull();
  });
});
describe("composer accessory and link effects", () => {
  it("composerViews.open opens the declared accessory for the bound composer", async () => {
    await applyAddonEffect(effect("composerViews.open", { id: "issues" }));
    expect(useAddonsStore.getState().accessory).toEqual({
      pluginId: "test.plugin",
      view: "issues",
      composerId: "composer",
      workspaceId: "workspace",
    });
    expect(lastResult().error).toBeNull();
  });
  it("composerViews.open never targets a composer that has closed", async () => {
    unregister();
    await applyAddonEffect(effect("composerViews.open", { id: "issues" }));
    expect(useAddonsStore.getState().accessory).toBeNull();
    expect(lastResult().error).toMatchObject({
      data: { code: "NO_COMPOSER" },
    });
  });
  it("links.open attributes the link to the add-on and opens it", async () => {
    await applyAddonEffect(
      effect("links.open", { url: "https://example.com/issues/1" }),
    );
    expect(toast.info).toHaveBeenCalledWith("Fixture is opening a link", {
      description: "https://example.com/issues/1",
    });
    expect(openUrl).toHaveBeenCalledExactlyOnceWith(
      "https://example.com/issues/1",
    );
    expect(lastResult().error).toBeNull();
  });
  it("links.open accepts any link the broker accepts and opens its normalized form", async () => {
    // The broker parses the URL, so the scheme's case and surrounding spaces
    // do not matter there; they must not be refused here either.
    await applyAddonEffect(
      effect("links.open", { url: " HTTPS://Example.com/issues/1 " }),
    );
    expect(toast.error).not.toHaveBeenCalled();
    expect(toast.info).toHaveBeenCalledWith("Fixture is opening a link", {
      description: "https://example.com/issues/1",
    });
    expect(openUrl).toHaveBeenCalledExactlyOnceWith(
      "https://example.com/issues/1",
    );
    expect(lastResult().error).toBeNull();
  });
  it("links.open reports a failed system open to the user and the plugin", async () => {
    vi.mocked(openUrl).mockRejectedValue("No handler");
    await applyAddonEffect(effect("links.open", { url: "https://example.com" }));
    expect(toast.error).toHaveBeenCalledWith(
      "Fixture couldn't open the link",
      expect.objectContaining({
        description: "The system could not open the link: No handler",
      }),
    );
    expect(lastResult().error).not.toBeNull();
  });
  it("panels.open activates the add-on's own pane in the bound workspace", async () => {
    useUIStore.setState({ rightPanelPanes: {}, rightPanelTabs: {} });
    await applyAddonEffect(effect("panels.open", { id: "brief" }));
    expect(useUIStore.getState().rightPanelTabs.workspace).toBe(
      "addon:test.plugin:brief",
    );
    expect(lastResult().error).toBeNull();
  });
});
describe("remote workspaces and clients", () => {
  it.each([
    ["host_id", { host_id: "host-1" }],
    ["remote_cwd", { remote_cwd: "/srv/project" }],
    ["attach_only", { attach_only: true }],
  ])(
    "a %s workspace exposes no add-on workspace and rejects its effects",
    async (_, remote) => {
      useAppStore.setState({
        appState: {
          active_workspace_id: "workspace",
          workspaces: [{ workspace_id: "workspace", ...remote }],
        } as unknown as AppStateSnapshot,
      });
      useUIStore.setState({ rightPanelPanes: {}, rightPanelTabs: {} });
      expect(activeAddonWorkspace()).toBeNull();
      await applyAddonEffect(event);
      expect(append).not.toHaveBeenCalled();
      expect(lastResult().error).toMatchObject({
        data: { code: "CONTEXT_STALE" },
      });
      await applyAddonEffect(effect("panels.open", { id: "brief" }));
      expect(useUIStore.getState().rightPanelTabs.workspace).toBeUndefined();
      expect(lastResult().error).toMatchObject({
        data: { code: "CONTEXT_STALE" },
      });
    },
  );
  it("a browser client never subscribes to add-ons or reads their inventory", () => {
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
    try {
      const hook = renderHook(() => useAddonPlatform());
      hook.unmount();
      expect(subscribeAddons).not.toHaveBeenCalled();
      expect(addonInventory).not.toHaveBeenCalled();
      expect(addonInvoke).not.toHaveBeenCalled();
    } finally {
      delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
    }
  });
});
describe("context races through the real composer adapter", () => {
  /** A composer bound to the workspace, and an appendText aimed at it whose
   *  claim is held until `release()`. */
  async function delayedInsertion() {
    const onDraftChange = vi.fn();
    const composer = renderHook(
      ({ threadId, draft }: { threadId: string; draft: string }) =>
        useAddonComposerAdapter("workspace", threadId, draft, onDraftChange),
      { initialProps: { threadId: "thread-1", draft: "unsent" } },
    );
    await waitFor(() => expect(composer.result.current.registered).toBe(true));
    const claim = deferred<unknown>();
    vi.mocked(addonInvoke).mockImplementation((command) =>
      command === "addon_effect_claim" ? claim.promise : Promise.resolve(null),
    );
    const pending = applyAddonEffect(
      effect("composer.appendText", {
        composerId: composer.result.current.id,
        text: "brief",
      }),
    );
    return {
      composer,
      onDraftChange,
      release: async () => {
        claim.resolve(null);
        await act(() => pending);
      },
    };
  }
  it("inserts after text the user typed while the request was delayed", async () => {
    const { composer, onDraftChange, release } = await delayedInsertion();
    composer.rerender({ threadId: "thread-1", draft: "unsent, then more" });
    await release();
    expect(onDraftChange).toHaveBeenCalledExactlyOnceWith(
      "unsent, then more\nbrief",
    );
    expect(lastResult().error).toBeNull();
    composer.unmount();
  });
  it("inserts nothing after a project switch", async () => {
    const { composer, onDraftChange, release } = await delayedInsertion();
    act(() => {
      useAppStore.setState({
        appState: {
          active_workspace_id: "other",
          workspaces: [{ workspace_id: "workspace" }, { workspace_id: "other" }],
        } as AppStateSnapshot,
      });
      clearAddonContext();
    });
    await release();
    expect(onDraftChange).not.toHaveBeenCalled();
    expect(lastResult().error).toMatchObject({ data: { code: "CONTEXT_STALE" } });
    composer.unmount();
  });
  it("inserts nothing after the thread closes", async () => {
    const { composer, onDraftChange, release } = await delayedInsertion();
    composer.unmount();
    await release();
    expect(onDraftChange).not.toHaveBeenCalled();
    expect(toast.error).toHaveBeenCalledWith(
      "Fixture couldn't add text to the draft",
      expect.objectContaining({
        description: "This chat composer is no longer available",
      }),
    );
  });
  it("inserts into neither draft after the surface moves to another thread", async () => {
    const { composer, onDraftChange, release } = await delayedInsertion();
    composer.rerender({ threadId: "thread-2", draft: "another thread" });
    await release();
    expect(onDraftChange).not.toHaveBeenCalled();
    expect(lastResult().error).toMatchObject({ data: { code: "NO_COMPOSER" } });
    composer.unmount();
  });
});
describe("saved add-on pane preferences", () => {
  const pane = "addon:test.plugin:brief" as const;
  const inventory = (overrides: Partial<AddonInventory>): AddonInventory => ({
    installed: [installation],
    paused: false,
    error: null,
    ...overrides,
  });
  const refreshWith = async (value: AddonInventory) => {
    vi.mocked(addonInventory).mockResolvedValueOnce(value);
    await refreshAddons();
  };
  beforeEach(() => {
    useUIStore.setState({
      rightPanelPanes: { workspace: ["files", pane], other: [pane] },
      rightPanelDismissedPanes: {},
      rightPanelTabs: { workspace: pane },
      rightPanelLastTabs: {},
    });
  });
  const saved = () => useUIStore.getState().rightPanelPanes;
  it("survive pause-all, a failed manager, a registry error and resume", async () => {
    await refreshWith(inventory({ paused: true }));
    await refreshWith(
      inventory({ paused: true, installed: [], error: "Manager unavailable" }),
    );
    await refreshWith(
      inventory({
        paused: true,
        installed: [],
        registryError: { path: "/data/addons-v1", cause: "locked" },
      }),
    );
    expect(saved()).toEqual({ workspace: ["files", pane], other: [pane] });
    expect(useUIStore.getState().rightPanelDismissedPanes).toEqual({});
    expect(useUIStore.getState().rightPanelTabs.workspace).toBe(pane);
    await refreshWith(inventory({}));
    expect(saved()).toEqual({ workspace: ["files", pane], other: [pane] });
  });
  it("survive an inventory request that fails outright", async () => {
    vi.mocked(addonInventory).mockRejectedValueOnce({ message: "IPC failed" });
    await refreshAddons();
    expect(saved()).toEqual({ workspace: ["files", pane], other: [pane] });
  });
  it("keep a failed add-on's pane so its diagnostic stays until closed", async () => {
    await refreshWith(
      inventory({
        installed: [
          { ...installation, status: "failed-disabled", failure: "Crashed" },
        ],
      }),
    );
    expect(saved()).toEqual({ workspace: ["files", pane], other: [pane] });
  });
  it.each<[string, AddonInstallation[]]>([
    [
      "disabled",
      [{ ...installation, desiredEnabled: false, status: "installed-disabled" }],
    ],
    [
      "disabled after a failure",
      [{ ...installation, desiredEnabled: false, status: "failed-disabled" }],
    ],
    ["removed", []],
    ["being removed", [{ ...installation, status: "removing" }]],
    ["blocked", [{ ...installation, status: "blocked-disabled" }]],
  ])("are forgotten everywhere once the add-on is %s", async (_, installed) => {
    await refreshWith(inventory({ installed }));
    expect(saved()).toEqual({ workspace: ["files"], other: [] });
    expect(useUIStore.getState().rightPanelDismissedPanes).toEqual({});
    expect(useUIStore.getState().rightPanelTabs.workspace).toBe("files");
  });
  it("forget a panel the installed release no longer declares", async () => {
    await refreshWith(
      inventory({
        installed: [
          {
            ...installation,
            manifest: {
              ...installation.manifest,
              contributes: { ...installation.manifest.contributes, panels: [] },
            },
          },
        ],
      }),
    );
    expect(saved()).toEqual({ workspace: ["files"], other: [] });
  });
});
