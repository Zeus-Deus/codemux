import { APP_OVERLAY_AREA, definePlugin, icons } from "@codemux/sdk";
import { desktopAdapter, peekAdapter, type PeekBodyProps } from "./adapter";
import { ComputerUseController } from "./controller";
import { ComputerIndicator, ComputerPreview, ComputerSettings, UnavailableSettings } from "./views";

export default definePlugin({
  id: "computer-use",
  register(ctx) {
    const desktop = desktopAdapter(ctx);
    const peek = peekAdapter(ctx);
    if (!desktop || !peek) {
      ctx.registerSettingsPage({ title: "Computer use", render: UnavailableSettings });
      return;
    }
    const controller = new ComputerUseController(desktop);
    const synchronizeWorkspace = () => {
      const workspace = ctx.host.workspace.current();
      const before = controller.getState();
      const optedIn = workspace ? ctx.storage.get<boolean>(`workspace.${workspace.workspace_id}`, false) === true : false;
      if (before.workspace?.id !== workspace?.workspace_id || before.optedIn !== optedIn) peek.close();
      void controller.setWorkspace(workspace ? {
        id: workspace.workspace_id, title: workspace.title,
        local: workspace.host_id == null && workspace.remote_cwd == null && !workspace.attach_only,
      } : null, optedIn);
    };
    synchronizeWorkspace();
    ctx.host.workspace.subscribe(synchronizeWorkspace);
    ctx.register({
      id: "live", area: peek.area, title: "Computer use", data: { icon: icons.Monitor },
      render: (props: PeekBodyProps) => <ComputerPreview {...props} controller={controller} />,
    });
    ctx.register({
      id: "stop", area: APP_OVERLAY_AREA,
      render: () => <ComputerIndicator controller={controller} peek={peek} />,
    });
    ctx.registerSettingsPage({
      title: "Computer use",
      render: () => <ComputerSettings controller={controller} peek={peek}
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
      peek.close();
      void controller.dispose().catch((error: unknown) => console.error("[computer-use] Could not confirm driver shutdown", error));
    });
  },
});
