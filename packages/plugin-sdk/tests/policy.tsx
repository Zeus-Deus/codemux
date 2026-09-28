import {
  definePlugin,
  Stack,
  Button,
  TextField,
  PluginError,
  useState,
} from "../src/index.js";
// Well-behaved and faulty callbacks for scripts/addons/sdk-native.mjs.
export default definePlugin({
  activate(ctx) {
    ctx.commands.register("handled", async () => {
      await ctx.ui.notify("handled");
    });
    ctx.commands.register("reject", async () => {
      await Promise.resolve();
      throw new Error("Synthetic plain rejection");
    });
    ctx.commands.register("throw", () => {
      throw new PluginError("NO_COMPOSER", "Synchronous throws stay faults");
    });
    const later = ctx.commands.register("later", async () => {
      await ctx.ui.notify("later");
    });
    ctx.commands.register("dispose", () => later());
    ctx.commands.register("burst", async () => {
      for (let n = 0; n < 40; n++)
        await ctx.storage.set({ scope: "global" }, "burst", n);
    });
    ctx.commands.register("exhaust", async () => {
      for (let n = 0; n < 70; n++)
        try {
          await ctx.storage.set({ scope: "global" }, "exhaust", n);
        } catch (error) {
          if (error instanceof PluginError)
            console.log("rejected " + error.code);
        }
    });
    ctx.commands.register("flood", () => {
      for (let n = 0; n < 50; n++) console.log("line " + n);
    });
    ctx.workspace.subscribe((context) => ctx.ui.notify("workspace " + context));
    const disposed = ctx.settings.subscribe(() =>
      ctx.ui.notify("disposed listener"),
    );
    disposed();
    ctx.settings.subscribe((settings) =>
      ctx.ui.notify("settings " + JSON.stringify(settings)),
    );
    ctx.panels.register("form", () => {
      const [text, setText] = useState("");
      return (
        <Stack>
          <TextField
            label="Echo"
            value={text}
            onChange={(event) => setText(event.value)}
          />
          <Button
            onPress={async (event) => {
              await ctx.composer.appendText(event.context, text);
            }}
          >
            Add
          </Button>
        </Stack>
      );
    });
    // Each press replaces the button, releasing the previous callback.
    ctx.panels.register("swap", () => {
      const [round, setRound] = useState(0);
      return (
        <Stack>
          <Button
            key={round}
            label={"Round " + round}
            onPress={() => setRound((n) => n + 1)}
          />
        </Stack>
      );
    });
    // A quarter of the plugin's 4,096 live callbacks.
    ctx.panels.register("grid", () => (
      <Stack>
        {Array.from({ length: 1024 }, (_, n) => (
          <Button key={n} label={String(n)} onPress={() => {}} />
        ))}
      </Stack>
    ));
  },
});
