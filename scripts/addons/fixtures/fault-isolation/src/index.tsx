import { definePlugin } from "@codemux/plugin-sdk";

// Deliberately hostile: never published or included in the catalog. This must
// execute exclusively in the same separate native host used by third parties.
export default definePlugin({
  activate(ctx) {
    ctx.commands.register("block", () => {
      while (true) {
        /* the native execution deadline must interrupt this */
      }
    });
  },
});
