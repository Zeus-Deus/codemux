import { definePlugin } from "@codemux/plugin-sdk";

// Deliberately hostile: never published or included in the catalog. This must
// execute exclusively in the same separate native host used by third parties.
export default definePlugin({
  activate(ctx) {
    ctx.commands.register("throw", () => {
      throw Error("Synthetic fixture exception");
    });
    ctx.commands.register("promises", () => {
      Promise.resolve().then(function loop() {
        Promise.resolve().then(loop);
      });
    });
    ctx.commands.register("recurse", () => {
      function recurse(): never {
        return recurse();
      }
      recurse();
    });
    ctx.commands.register("allocate", () => {
      new ArrayBuffer(128 * 1024 * 1024);
    });
    ctx.commands.register("block", () => {
      while (true) {
        /* the native execution deadline must interrupt this */
      }
    });
  },
});
