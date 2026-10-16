/**
 * `host`: the app's API for plugins. Full authority; members here are the
 * stable surface, `host.invoke`/`host.listen` the unstable raw door.
 */
import { toast } from "@/lib/toast";
import type { ComponentType } from "react";

export interface PaletteCommandData {
  label: string;
  /** Lucide-style icon component. */
  icon?: ComponentType<{ className?: string }>;
  keywords?: string;
  /** Hidden when there is no active workspace. */
  requiresWorkspace?: boolean;
  run: (ctx: { workspaceId: string | null }) => void;
}

export type NotifyKind = "info" | "success" | "warning" | "error";

function notify({ kind = "info", message, description }: { kind?: NotifyKind; message: string; description?: string }) {
  toast[kind](message, description ? { description } : undefined);
}

export const host = {
  notify,
};
