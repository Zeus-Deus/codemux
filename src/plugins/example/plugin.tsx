import { definePlugin, host, PALETTE_AREA } from "@codemux/sdk";

export default definePlugin({
  id: "example",
  name: "Example",
  register(ctx) {
    ctx.register({
      id: "hello",
      area: PALETTE_AREA,
      data: {
        label: "Example plugin: say hello",
        run: () => host.notify({ message: "Hello from the bundled example plugin" }),
      },
    });
  },
});
