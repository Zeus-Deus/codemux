import { definePlugin, Stack, Text, Button, useState } from "../src/index.js";
export default definePlugin({
  activate(ctx) {
    ctx.commands.register("hello", async () => {
      await ctx.ui.notify("hello");
    });
    ctx.panels.register("test", () => {
      const [count, setCount] = useState(0);
      return (
        <Stack spacing="md" direction={count === 0 ? "horizontal" : undefined}>
          <Text>Clicks: {count}</Text>
          <Button
            onPress={async (event) => {
              await ctx.composer.appendText(event.context, "From SDK");
              setCount((c) => c + 1);
            }}
          >
            Add
          </Button>
        </Stack>
      );
    });
  },
});
