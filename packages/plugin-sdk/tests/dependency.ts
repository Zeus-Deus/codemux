import { definePlugin } from "../src/index.js";
// A CommonJS package with only a `main` field that reads process.env.NODE_ENV;
// the native test provides it in a temporary node_modules directory.
// @ts-expect-error: not a dependency of the SDK package
import dependency from "main-only";
export default definePlugin({
  activate(ctx) {
    ctx.commands.register("hello", () => ctx.ui.notify(dependency.mode));
  },
});
