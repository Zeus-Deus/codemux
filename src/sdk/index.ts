/**
 * `@codemux/sdk`: the one import a plugin needs. Bundled plugins resolve it
 * through the Vite alias; disk plugins get the same live object through the
 * runtime shim (sdk/runtime.ts).
 *
 * - areas: string constants for `ctx.register({ area })`
 * - host: the app API (theme, navigation, workspace, settings, composer, chat, raw IPC)
 * - UI kit: the app's own components, `cn`, and lucide icons as `icons`
 * - React state helpers: zustand's `create`, React Query hooks
 */
export * from "./areas";
export { host } from "./host";
export * from "./ui";
export * as icons from "lucide-react";
export { create } from "zustand";
export { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
/** The engraved plates the empty sidebar draws, and the one it showed last. */
export { PLATES, readLastPlate, type PlateEntry } from "@/lib/plates";

export type {
  CodemuxPlugin,
  PluginContext,
  PluginContribution,
  PluginSettingsPage,
  PluginStorage,
} from "@/contrib/plugin";
export type { Contribution } from "@/contrib/types";
export type { BundledPluginMeta } from "@/contrib/bundled";
export type { PluginChatEvent } from "@/contrib/chat-events";
export type { CompactionRefusal, CompactionRequestResult, ThreadStatus } from "@/contrib/ipc";
export type { Host } from "./host";
export type {
  ComposerActionData,
  ComposerApi,
  KeybindData,
  LandingBackdropProps,
  NotifyKind,
  PaletteCommandData,
  RightPaneData,
  ThreadSnapshot,
  ToolCallView,
  ToolRendererData,
} from "./host";
export type { ThemeDefinition, ThemeRole } from "@/lib/themes";
export type { ProviderRuntimeEvent } from "@/tauri/events";

/** Identity helper for typed plugin definitions. */
export function definePlugin<T extends import("@/contrib/plugin").CodemuxPlugin>(plugin: T): T {
  return plugin;
}
