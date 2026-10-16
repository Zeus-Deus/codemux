/**
 * `@codemux/sdk`: the one import a plugin needs. Bundled plugins resolve it
 * through the Vite alias; disk plugins get the same live object through the
 * runtime shim (sdk/runtime.ts).
 */
export * from "./areas";
export { host } from "./host";
export type {
  CodemuxPlugin,
  PluginContext,
  PluginContribution,
  PluginSettingsPage,
  PluginStorage,
} from "@/contrib/plugin";
export type { Contribution } from "@/contrib/types";
export type { PaletteCommandData } from "./host";

/** Identity helper for typed plugin definitions. */
export function definePlugin<T extends import("@/contrib/plugin").CodemuxPlugin>(plugin: T): T {
  return plugin;
}
