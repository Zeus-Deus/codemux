import { useCallback, useEffect } from "react";
import { useActiveWorkspaceId } from "@/stores/app-store";
import { useBrowserPeekStore, type PeekStatusKind } from "@/stores/browser-peek-store";
import { useContributions } from "@/contrib/react/use-contributions";
import { ContribBoundary, ContribRender } from "@/contrib/react/boundary";
import { registry } from "@/contrib/registry";
import type { PeekData, PeekRenderProps } from "@/contrib/peek";
import { PEEK_AREA, RIGHT_PANEL_AREA } from "@/sdk/areas";
import { host } from "@/sdk/host";
import { PeekShell, type PeekContent } from "./PeekShell";

export function PluginPeekOverlay() {
  const workspaceId = useActiveWorkspaceId();
  const peek = useBrowserPeekStore((s) => s.pluginPeek);
  const browserOpen = useBrowserPeekStore((s) => s.openWorkspaceId !== null);
  const items = useContributions(PEEK_AREA);
  const panes = useContributions(RIGHT_PANEL_AREA);
  const contribution = items.find((c) => c.id === peek?.id && c.render);
  const generation = peek?.generation;
  const onStatusChange = useCallback((status: PeekStatusKind, label?: string) => {
    if (generation !== undefined) useBrowserPeekStore.getState().setPluginStatus(generation, status, label);
  }, [generation]);
  useEffect(() => {
    if (peek && peek.workspaceId !== workspaceId) useBrowserPeekStore.getState().closePlugin(peek.id);
  }, [peek, workspaceId]);
  let content: PeekContent | null = null;
  if (peek && workspaceId === peek.workspaceId && contribution?.render) {
    const data = contribution.data as PeekData | undefined;
    const close = () => useBrowserPeekStore.getState().closePlugin(peek.id);
    const dockPane = panes.find((c) => c.id === data?.dockPaneId && c.source === contribution.source);
    content = {
      id: peek.id,
      workspaceId,
      title: contribution.title ?? "Preview",
      ariaLabel: `${contribution.title ?? "Plugin"} preview`,
      icon: data?.icon,
      status: peek.status,
      statusLabel: peek.statusLabel,
      close,
      available: () => registry.getArea(PEEK_AREA).includes(contribution),
      dock: dockPane ? () => { if (host.openRightPane(dockPane.id)) close(); } : undefined,
      render: ({ size }) => <ContribBoundary id={contribution.id} variant="block">
        <ContribRender<PeekRenderProps> render={contribution.render!} props={{ workspaceId, size, close, onStatusChange }} />
      </ContribBoundary>,
    };
  }
  return <PeekShell content={content} workspaceId={workspaceId} allowExit={!browserOpen} />;
}
