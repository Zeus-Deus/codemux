import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { UtilityPage } from "@/components/layout/utility-page";
import { useActiveWorkspace, useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import type { PluginInstallation } from "./contract";
import { findPluginView, pluginInactiveReason } from "./model";
import { openPluginSettings } from "./plugin-actions";
import { usePlatformCatalogue } from "./platform-adapter";
import { PluginViewFrame, PluginViewNotice } from "./PluginViewFrame";
import { workspaceActiveThreadId } from "./workspace-context";

function unavailableReason(
  catalogue: ReturnType<typeof usePlatformCatalogue>,
  installation: PluginInstallation | null,
): string | null {
  if (!catalogue.loaded) return null;
  if (catalogue.catalogue === null) return catalogue.error ?? "Plugins are unavailable.";
  if (catalogue.catalogue.unavailable) return catalogue.catalogue.unavailable.message;
  if (installation === null) return "The plugin was removed.";
  if (catalogue.catalogue.executionDisabled === true) return "Plugin code is turned off, so its views cannot open.";
  return (
    pluginInactiveReason(installation) ??
    (installation.grants.capabilities.includes("ui") ? null : "The plugin was not allowed to show views.")
  );
}

/** Resolves the installation live, so a view never outlives the plugin's enablement. */
function PluginViewResolver(props: {
  readonly installationId: string;
  readonly viewId: string;
  readonly placement: "page" | "side-panel";
  readonly workspaceId: string | null;
  readonly threadId: string | null;
}) {
  const catalogue = usePlatformCatalogue();
  const entry = findPluginView(catalogue.catalogue?.installations ?? [], props.installationId, props.viewId);
  const reason = unavailableReason(catalogue, entry?.installation ?? null);
  if (reason !== null) {
    return (
      <PluginViewNotice
        title="View unavailable"
        description={reason}
        action={
          <Button size="sm" variant="outline" onClick={() => openPluginSettings(props.installationId)}>
            Open plugin settings
          </Button>
        }
      />
    );
  }
  if (entry !== null && entry.view.placement === "conversation") {
    return <PluginViewNotice title="View unavailable" description="This view only opens from a card in chat." />;
  }
  if (entry === null) {
    return (
      <div className="flex size-full items-center justify-center">
        <Spinner />
      </div>
    );
  }
  return (
    <PluginViewFrame
      key={entry.installation.generation}
      installation={entry.installation}
      view={entry.view}
      placement={props.placement}
      workspaceId={props.workspaceId}
      threadId={props.threadId}
    />
  );
}

/** A plugin page in the utility-page area, opened by id `@platform:<installation>:<view>`. */
export function PlatformPluginPage({ installationId, viewId }: { installationId: string; viewId: string }) {
  const catalogue = usePlatformCatalogue();
  const entry = findPluginView(catalogue.catalogue?.installations ?? [], installationId, viewId);
  const close = () => useUIStore.getState().setShowPluginPage(null);
  // A side panel shown full screen (the phone layout) keeps its chat context.
  const workspace = useActiveWorkspace();
  const besideChat = entry?.view.placement === "side-panel" && workspace !== null;
  return (
    <UtilityPage title={entry?.view.title ?? "Plugin"} backLabel="Close page" onBack={close}>
      <div className="h-full min-h-0" data-platform-plugin-page={`${installationId}:${viewId}`}>
        <PluginViewResolver
          installationId={installationId}
          viewId={viewId}
          placement={besideChat ? "side-panel" : "page"}
          workspaceId={besideChat ? workspace.workspace_id : null}
          threadId={besideChat ? workspaceActiveThreadId(workspace) : null}
        />
      </div>
    </UtilityPage>
  );
}

/**
 * A side-panel view in a workspace's right panel, beside the chat the
 * workspace shows. Registered by installation, so it follows that
 * installation's live generation.
 */
export function PlatformSidePanel({
  installationId,
  viewId,
  workspaceId,
}: {
  installationId: string;
  viewId: string;
  workspaceId: string;
}) {
  const catalogue = usePlatformCatalogue();
  const installation =
    catalogue.catalogue?.installations.find((entry) => entry.installationId === installationId) ?? null;
  const workspace = useAppStore((s) => s.appState?.workspaces.find((ws) => ws.workspace_id === workspaceId) ?? null);
  const threadId = workspaceActiveThreadId(workspace);
  if (installation === null) {
    return <PluginViewNotice title="View unavailable" description="The plugin was removed." />;
  }
  return (
    <PluginViewResolver
      key={installation.installationId}
      installationId={installation.installationId}
      viewId={viewId}
      placement="side-panel"
      workspaceId={workspaceId}
      threadId={threadId}
    />
  );
}
