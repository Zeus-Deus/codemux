import { useEffect } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { selectActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import {
  addonTreeKey,
  clearAddonContext,
  useAddonsStore,
} from "@/stores/addons-store";
import { addonInventory, addonInvoke, subscribeAddons } from "./bridge";
import { addonComposer, composerForWorkspace } from "./composer-registry";
import {
  addonEnabled,
  addonError,
  addonMessage,
  type AddonEvent,
  type AddonInventory,
} from "./types";
export function activeAddonWorkspace(): string | null {
  const state = useAppStore.getState();
  const id = selectActiveWorkspaceId(state);
  const workspace = state.appState?.workspaces.find(
    (w) => w.workspace_id === id,
  );
  return workspace &&
    !workspace.host_id &&
    !workspace.remote_cwd &&
    !workspace.attach_only
    ? id
    : null;
}
/**
 * Saved add-on pane IDs are dropped only when the user disabled or removed
 * the add-on, or its release no longer declares the panel. Pause-all, a
 * diagnostic launch without add-ons, recovery and a registry failure keep
 * them: the right panel hides unavailable panes while they last, and they
 * come back on resume or repair.
 */
function forgetUnwantedAddonPanes(inventory: AddonInventory) {
  if (inventory.paused || inventory.error || inventory.registryError) return;
  const wanted = new Set(
    inventory.installed
      .filter(
        (i) =>
          i.desiredEnabled &&
          i.status !== "removing" &&
          i.status !== "blocked-disabled",
      )
      .flatMap(({ manifest }) =>
        manifest.contributes.panels.map(
          (panel) => `addon:${manifest.id}:${panel.id}`,
        ),
      ),
  );
  useUIStore
    .getState()
    .forgetRightPanelPanes(
      (pane) => pane.startsWith("addon:") && !wanted.has(pane),
    );
}
let inventoryRevision = 0;
let latestInventory: Promise<void> = Promise.resolve();
export function refreshAddons(): Promise<void> {
  const revision = ++inventoryRevision;
  const request = (async () => {
    try {
      const inventory = await addonInventory();
      if (revision !== inventoryRevision) return;
      useAddonsStore.setState({ ...inventory, loaded: true });
      forgetUnwantedAddonPanes(inventory);
    } catch (error) {
      if (revision !== inventoryRevision) return;
      useAddonsStore.setState({
        loaded: true,
        error: addonMessage(error),
        paused: true,
      });
    }
  })();
  const complete: Promise<void> = request.then(() =>
    latestInventory === complete ? undefined : latestInventory,
  );
  latestInventory = complete;
  return complete;
}

export async function applyAddonEffect(
  event: Extract<AddonEvent, { type: "effect" }>,
) {
  let value: unknown = null;
  let error = null;
  try {
    const before = useAddonsStore.getState().contextRevision;
    await addonInvoke("addon_effect_claim", {
      requestId: event.requestId,
      generation: event.generation,
    });
    const state = useAddonsStore.getState();
    const installation = state.installed.find(
      (i) => i.manifest.id === event.pluginId,
    );
    if (
      state.paused ||
      state.revoking["*"] ||
      state.revoking[event.pluginId] ||
      !installation ||
      !addonEnabled(installation) ||
      state.failures[event.generation]
    )
      throw addonError("PLUGIN_STOPPED", "Add-on stopped");
    const p = event.params;
    if (
      event.operation !== "ui.notify" &&
      (before !== state.contextRevision ||
        !state.ready ||
        p.workspaceId !== activeAddonWorkspace())
    )
      throw addonError("CONTEXT_STALE", "The active workspace changed");
    switch (event.operation) {
      case "composer.appendText": {
        if (!useFeatureFlags.getState().enableAgentChat)
          throw addonError("NO_COMPOSER", "Chat GUI is disabled");
        value = addonComposer(
          String(p.composerId),
          String(p.workspaceId),
        ).append(String(p.text));
        break;
      }
      case "composerViews.open": {
        if (!useFeatureFlags.getState().enableAgentChat)
          throw addonError("NO_COMPOSER", "Chat GUI is disabled");
        addonComposer(String(p.composerId), String(p.workspaceId));
        useAddonsStore.setState({
          accessory: {
            pluginId: event.pluginId,
            view: String(p.id),
            composerId: String(p.composerId),
            workspaceId: String(p.workspaceId),
          },
        });
        break;
      }
      case "panels.open":
        useUIStore
          .getState()
          .setRightPanelTab(
            String(p.workspaceId),
            `addon:${event.pluginId}:${String(p.id)}`,
          );
        break;
      case "links.open":
        toast.info(`${installation.manifest.name}: opening external link`);
        await openUrl(String(p.url));
        break;
      case "ui.notify":
        toast.info(installation.manifest.name, {
          description: String(p.message),
        });
        break;
      default:
        throw addonError("INVALID_MESSAGE", "Unknown add-on UI operation");
    }
  } catch (cause) {
    error =
      typeof cause === "object" && cause !== null && "data" in cause
        ? cause
        : addonError("CONTEXT_STALE", addonMessage(cause));
  }
  await addonInvoke("addon_effect_result", {
    requestId: event.requestId,
    generation: event.generation,
    value,
    error,
  }).catch(() => {});
}
function receive(event: AddonEvent) {
  if (event.type === "development-review") {
    useAddonsStore.setState({ developmentReview: event.review });
    toast.info("Development package needs access review", {
      description: "Open Settings → Add-ons to review the changed permissions.",
    });
  }
  if (event.type === "development-error")
    toast.error("Development reload failed", { description: event.message });
  if (event.type === "inventory") void refreshAddons();
  if (event.type === "effect") void applyAddonEffect(event);
  if (event.type === "tree")
    useAddonsStore.setState((state) =>
      state.failures[event.generation] || !state.ready
        ? {}
        : {
            trees: {
              ...state.trees,
              [addonTreeKey(event.generation, event.viewId)]: event,
            },
          },
    );
  if (event.type === "stopped")
    useAddonsStore.setState((state) => ({
      hostEpochs: {
        ...state.hostEpochs,
        [event.pluginId]: (state.hostEpochs[event.pluginId] ?? 0) + 1,
      },
      failures: Object.fromEntries([
        ...Object.entries(state.failures).slice(-63),
        [event.generation, event.message],
      ]),
      trees: Object.fromEntries(
        Object.entries(state.trees).filter(
          ([, v]) => v.generation !== event.generation,
        ),
      ),
      accessory:
        state.accessory?.pluginId === event.pluginId ? null : state.accessory,
    }));
}
export async function executeAddon(
  id: string,
  command: string,
  kind: "commands" | "composerActions",
  composerId?: string,
) {
  const workspaceId = activeAddonWorkspace();
  try {
    await addonInvoke("addon_execute", {
      id,
      command,
      kind,
      workspaceId,
      composerId:
        composerId ?? (workspaceId ? composerForWorkspace(workspaceId) : null),
    });
  } catch (error) {
    toast.error("Add-on action unavailable", {
      description: addonMessage(error),
    });
  }
}
export function useAddonPlatform() {
  useEffect(() => {
    if (isRemoteClient()) return;
    let disposed = false;
    let selection = activeAddonWorkspace();
    let chain = Promise.resolve();
    const context = () => {
      clearAddonContext();
      const revision = useAddonsStore.getState().contextRevision;
      chain = chain
        .catch(() => {})
        .then(async () => {
          await addonInvoke("addon_context_changed", {
            workspaceId: activeAddonWorkspace(),
          }).catch(() => {});
          if (
            !disposed &&
            revision === useAddonsStore.getState().contextRevision
          )
            useAddonsStore.setState({ ready: true });
        });
    };
    void subscribeAddons((event) => {
      if (!disposed) receive(event);
    })
      .then(() => {
        if (!disposed) {
          context();
          void refreshAddons();
        }
      })
      .catch((error) =>
        useAddonsStore.setState({
          error: addonMessage(error),
          loaded: true,
          paused: true,
        }),
      );
    const unsubscribe = useAppStore.subscribe(() => {
      const next = activeAddonWorkspace();
      if (next !== selection) {
        selection = next;
        context();
      }
    });
    return () => {
      disposed = true;
      unsubscribe();
      clearAddonContext();
    };
  }, []);
}
