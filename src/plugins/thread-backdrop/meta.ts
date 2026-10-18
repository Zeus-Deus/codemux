import type { BundledPluginMeta } from "@codemux/sdk";

const meta: BundledPluginMeta = {
  id: "thread-backdrop",
  name: "Thread backdrop",
  description:
    "An engraving typed out in faint monospace behind a new thread. Animates once when the thread opens and when you click empty space, then sits still.",
  version: "1.0.0",
  defaultEnabled: true,
};

export default meta;
