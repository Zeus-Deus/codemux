// Synthetic author package for installed-desktop race tests. Retain the exact
// bundled public SDK from the independently built example in the input artifact.
// Only replace its authored plugin definition and declared contributions.
import assert from "node:assert/strict";
import { transformFixture } from "./reversion-fixture.mjs";
export const contextCases = [
  "typing",
  "workspace",
  "thread",
  "replacement",
  "disable",
];
export function contextFixture(bytes) {
  const seen = new Set();
  const result = transformFixture(bytes, (name, data) => {
    if (name === "manifest.json") {
      seen.add(name);
      const manifest = JSON.parse(data);
      assert.equal(manifest.id, "codemux.project-brief");
      Object.assign(manifest, {
        id: "example.context-races",
        name: "Context Race Fixture",
        description:
          "Synthetic delayed public SDK draft operations for disposable CI.",
        permissions: ["composer.append"],
        settings: [],
        http: [],
        credentials: [],
        contributes: {
          commands: contextCases.map((id) => ({
            id,
            title: `CI delayed ${id}`,
            requiresWorkspace: true,
          })),
          panels: [],
          composerActions: [],
          composerViews: [],
        },
      });
      return Buffer.from(JSON.stringify(manifest, null, 2));
    }
    if (name === "plugin.js") {
      seen.add(name);
      const source = data.toString();
      const entry = "register(src_default);";
      assert.equal(
        source.split(entry).length,
        2,
        "Expected one public SDK entrypoint",
      );
      return Buffer.from(
        source.replace(
          entry,
          `register({ activate(ctx) {
        for (const id of ${JSON.stringify(contextCases)}) {
          ctx.commands.register(id, async context => {
            await ctx.ui.notify("CI pending " + id);
            await new Promise(resolve => setTimeout(resolve, 4000));
            try {
              await ctx.composer.appendText(context, " CI delayed " + id + ".");
              await ctx.ui.notify("CI appended " + id);
            } catch (error) {
              await ctx.ui.notify("CI cancelled " + id + ": " + error.code);
            }
          });
        }
      }});`,
        ),
      );
    }
    return data;
  });
  assert.deepEqual([...seen].sort(), ["manifest.json", "plugin.js"]);
  return result;
}
