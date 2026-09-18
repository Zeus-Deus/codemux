import { useEffect, useState } from "react";
import { addonTreeKey, useAddonsStore } from "@/stores/addons-store";
import { addonInvoke, mountAddon } from "@/lib/addons/bridge";
import { composerForWorkspace } from "@/lib/addons/composer-registry";
import {
  addonMessage,
  type AddonMount,
  type AddonNode,
} from "@/lib/addons/types";
import { AddonRenderer } from "./addon-renderer";
export function AddonView({
  id,
  view,
  workspaceId,
  kind = "panels",
  composerId,
}: {
  id: string;
  view: string;
  workspaceId: string;
  kind?: "panels" | "composerViews";
  composerId?: string;
}) {
  const ready = useAddonsStore((s) => s.ready);
  const revision = useAddonsStore((s) => s.contextRevision);
  const installation = useAddonsStore((s) =>
    s.installed.find((i) => i.manifest.id === id),
  );
  const [mounted, setMounted] = useState<AddonMount | null>(null);
  const [error, setError] = useState<string | null>(null);
  const tree = useAddonsStore((s) =>
    mounted
      ? s.trees[addonTreeKey(mounted.generation, mounted.viewId)]
      : undefined,
  );
  const failed = useAddonsStore((s) =>
    mounted ? s.failures[mounted.generation] : undefined,
  );
  useEffect(() => {
    setMounted(null);
    setError(null);
    if (!ready || installation?.status === "failed-disabled") return;
    let closed = false;
    let target: AddonMount | null = null;
    const unmount = () => {
      if (target) {
        const current = target;
        void addonInvoke("addon_unmount", { id, ...current }).catch(() => {});
        useAddonsStore.setState((s) => ({
          trees: Object.fromEntries(
            Object.entries(s.trees).filter(
              ([key]) =>
                key !== addonTreeKey(current.generation, current.viewId),
            ),
          ),
        }));
      }
    };
    void mountAddon(
      id,
      view,
      kind,
      workspaceId,
      composerId ?? composerForWorkspace(workspaceId),
    )
      .then((result) => {
        target = result;
        if (closed) unmount();
        else setMounted(result);
      })
      .catch((cause) => {
        if (!closed) setError(addonMessage(cause));
      });
    return () => {
      closed = true;
      unmount();
    };
  }, [
    id,
    view,
    workspaceId,
    kind,
    composerId,
    ready,
    revision,
    installation?.status === "failed-disabled",
  ]);
  useEffect(() => {
    if (tree)
      void addonInvoke("addon_ui_ack", {
        id,
        generation: tree.generation,
        viewId: tree.viewId,
        revision: tree.revision,
      }).catch(() => {});
  }, [tree, id]);
  function event(
    node: AddonNode,
    event: "press" | "change",
    value: string | boolean | null,
  ) {
    const callbackId = node.eventListeners[event]?.callbackId;
    if (!mounted || !callbackId) return;
    void addonInvoke("addon_ui_event", {
      id,
      ...mounted,
      nodeId: node.id,
      event,
      callbackId,
      value,
    }).catch((cause) => setError(addonMessage(cause)));
  }
  return (
    <section
      aria-label="Add-on view"
      className="h-full min-h-0 overflow-auto p-3"
    >
      {error || failed || installation?.status === "failed-disabled" ? (
        <div role="alert" className="rounded-md border p-3 text-body">
          <p className="font-medium">Add-on unavailable</p>
          <p className="mt-1 text-muted-foreground">
            {error ||
              failed ||
              installation?.failure ||
              "The add-on stopped. Retry or disable it in Settings → Add-ons."}
          </p>
        </div>
      ) : tree ? (
        <AddonRenderer
          nodes={tree.tree.children}
          event={event}
          link={(node, url) => {
            if (mounted)
              void addonInvoke("addon_ui_link", {
                id,
                ...mounted,
                nodeId: node.id,
                url,
              }).catch((cause) => setError(addonMessage(cause)));
          }}
        />
      ) : (
        <p role="status" className="text-body text-muted-foreground">
          Loading add-on…
        </p>
      )}
    </section>
  );
}
