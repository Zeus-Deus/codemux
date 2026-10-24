import { PEEK_AREA, COMPOSER_STATUS_AREA, TITLEBAR_ACTIONS_AREA, definePlugin, icons, type ComposerStatusData, type ComposerStatusProps, type PeekRenderProps } from "@codemux/sdk";
import { ComputerUseController } from "./controller";
import { ComputerChatChip, ComputerIndicator, ComputerPreview, ComputerSettings, computerChipVisible } from "./views";

export default definePlugin({
  id: "computer-use",
  register(ctx) {
    const peek = ctx.host.peek;
    const controller = new ComputerUseController({ ...ctx.host, registerMcpServer: ctx.registerMcpServer });
    void controller.sweepStaleFiles();
    const synchronizeWorkspace = () => {
      const workspace = ctx.host.workspace.current();
      const before = controller.getState();
      const optedIn = workspace ? ctx.storage.get<boolean>(`workspace.${workspace.workspace_id}`, false) === true : false;
      if (before.workspace?.id !== workspace?.workspace_id || before.optedIn !== optedIn) peek.close(`${ctx.id}:live`);
      void controller.setWorkspace(workspace ? {
        id: workspace.workspace_id, title: workspace.title,
        local: workspace.host_id == null && workspace.remote_cwd == null && !workspace.attach_only,
      } : null, optedIn);
    };
    synchronizeWorkspace();
    ctx.host.workspace.subscribe(synchronizeWorkspace);
    ctx.register({
      id: "live", area: PEEK_AREA, title: "Computer use", data: { icon: icons.Monitor },
      render: (props: PeekRenderProps) => <ComputerPreview {...props} controller={controller} />,
    });
    ctx.register({
      id: "stop", area: TITLEBAR_ACTIONS_AREA,
      render: () => <ComputerIndicator controller={controller} peek={peek} showWorkspace={ctx.host.showWorkspace} />,
    });
    ctx.register({
      id: "status", area: COMPOSER_STATUS_AREA,
      render: ({ workspaceId }: ComposerStatusProps) =>
        <ComputerChatChip controller={controller} workspaceId={workspaceId} />,
      data: {
        subscribe: controller.subscribe,
        isVisible: ({ workspaceId }) => computerChipVisible(controller.getState(), workspaceId),
      } satisfies ComposerStatusData,
    });
    ctx.registerSettingsPage({
      title: "Computer use",
      render: () => <ComputerSettings controller={controller} peek={peek} showWorkspace={ctx.host.showWorkspace}
        initialCommand={ctx.storage.get("executable", "")}
        onCommand={(command) => ctx.storage.set("executable", command)}
        onOptIn={(value) => {
          const workspace = ctx.host.workspace.current();
          if (!workspace || workspace.host_id != null || workspace.remote_cwd != null || workspace.attach_only) return;
          ctx.storage.set(`workspace.${workspace.workspace_id}`, value);
          synchronizeWorkspace();
        }} />,
    });
    ctx.onDispose(() => {
      peek.close(`${ctx.id}:live`);
      void controller.dispose().catch((error: unknown) => console.error("[computer-use] Could not confirm driver shutdown", error));
    });
  },
});
