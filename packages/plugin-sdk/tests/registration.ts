import { definePlugin } from "../src/index.js";
// The native test bundles this once per CASE; each variant must fail activation.
declare const CASE: "undeclared" | "duplicate" | "missing";
export default definePlugin({
  activate(ctx) {
    if (CASE !== "missing") ctx.commands.register("hello", () => {});
    if (CASE === "duplicate") ctx.commands.register("hello", () => {});
    if (CASE === "undeclared") ctx.commands.register("other", () => {});
  },
});
