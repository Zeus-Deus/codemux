import { createElement as h, useEffect } from "react";
import { PEEK_AREA, RIGHT_PANEL_AREA, COMPOSER_STATUS_AREA, PeekChip, icons } from "@codemux/sdk";

function Viewer({ workspaceId, onStatusChange }) {
  useEffect(() => { onStatusChange?.("live", "Demo view ready"); }, [onStatusChange]);
  return h("div", {
    className: "flex h-full flex-col items-center justify-center gap-3 bg-background p-4 text-muted-foreground",
  }, h(icons.Monitor, { className: "size-8" }),
  h("p", { className: "text-body font-medium text-foreground" }, "Plugin preview"),
  h("p", { className: "text-caption" }, `Workspace: ${workspaceId}`));
}

function StatusChip() {
  return h(PeekChip, { peekId: "peek-view:live", label: "Preview", icon: icons.Monitor, status: "live" });
}

export default {
  id: "peek-view",
  name: "Peek view example",
  description: "A shared floating preview and compact composer chip",
  version: "0.1.0",
  register(ctx) {
    ctx.registerMany([
      { id: "pane", area: RIGHT_PANEL_AREA, title: "Plugin preview", data: { icon: icons.Monitor }, render: Viewer },
      { id: "live", area: PEEK_AREA, title: "Plugin preview", data: { icon: icons.Monitor, dockPaneId: `${ctx.id}:pane` }, render: Viewer },
      { id: "chip", area: COMPOSER_STATUS_AREA, render: StatusChip },
    ]);
  },
};
