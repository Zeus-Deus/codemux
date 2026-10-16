// A minimal Codemux disk plugin: one palette command and one right-panel
// pane. Plain ESM; it may import only "@codemux/sdk" and "react".
import * as React from "react";
import { Button, host, icons, PALETTE_AREA, RIGHT_PANEL_AREA } from "@codemux/sdk";

const VERSION = "0.1.0";
const h = React.createElement;

function HelloPane({ workspaceId }) {
  const [count, setCount] = React.useState(0);
  return h(
    "div",
    { style: { padding: 16, display: "grid", gap: 12 } },
    h("strong", null, `Hello ${VERSION}`),
    h("span", null, workspaceId ? "A workspace is open." : "No workspace."),
    h(Button, { size: "sm", variant: "outline", onClick: () => setCount((n) => n + 1) }, `Waved ${count} times`),
  );
}

export default {
  id: "hello",
  name: "Hello",
  description: "Says hello from the palette and a right-panel pane",
  version: VERSION,
  register(ctx) {
    ctx.registerMany([
      {
        id: "say",
        area: PALETTE_AREA,
        data: {
          label: `Hello: say hello (${VERSION})`,
          icon: icons.Hand,
          run: () => host.notify({ message: `Hello from the example plugin ${VERSION}` }),
        },
      },
      {
        id: "open",
        area: PALETTE_AREA,
        data: {
          label: "Hello: open pane",
          requiresWorkspace: true,
          run: () => host.openRightPane("hello:pane"),
        },
      },
      { id: "pane", area: RIGHT_PANEL_AREA, title: "Hello", data: { icon: icons.Hand }, render: HelloPane },
    ]);
  },
};
