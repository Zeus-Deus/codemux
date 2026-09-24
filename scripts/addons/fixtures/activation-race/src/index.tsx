import { definePlugin } from "@codemux/plugin-sdk";

// CI-only: activation waits (without using active JS time) so the harness can
// remove the add-on while its native host is still starting. If removal ever
// lost that race, the command would append to the draft.
export default definePlugin({
  async activate(ctx) {
    await new Promise((resolve) => setTimeout(resolve, 700));
    ctx.commands.register("append", async (context) => {
      await ctx.composer.appendText(context, " CI activation race.");
    });
  },
});
