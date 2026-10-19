import { useSyncExternalStore, type ComponentType } from "react";
import { host } from "@/sdk/host";
import { useBrowserPeekStore, type PeekStatusKind } from "@/stores/browser-peek-store";
import { registry } from "@/contrib/registry";
import { PEEK_AREA } from "@/sdk/areas";
import { PeekStatus } from "./PeekStatus";

export function PeekChip({ peekId, label, icon: Icon, status }: {
  peekId: string;
  label: string;
  icon?: ComponentType<{ className?: string }>;
  status?: PeekStatusKind;
}) {
  const items = useSyncExternalStore(
    (run) => registry.subscribeArea(PEEK_AREA, run),
    () => registry.getArea(PEEK_AREA),
  );
  const open = useBrowserPeekStore((s) => s.pluginPeek?.id === peekId);
  if (!items.some((c) => c.id === peekId && c.render)) return null;
  return <button type="button" aria-label={`View ${label} preview`} aria-pressed={open}
    onClick={() => host.peek.toggle(peekId)}
    className="inline-flex h-[26px] shrink-0 items-center gap-1.5 px-1.5 text-label font-semibold text-status-remote transition-opacity duration-150 hover:opacity-80 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-status-remote/50">
    {Icon && <Icon className="size-3.5" />}
    {status && <PeekStatus status={status} />}
    {label}
  </button>;
}
