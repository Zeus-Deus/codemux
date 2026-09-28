import { useMemo } from "react";
import { Puzzle, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useAddonsStore } from "@/stores/addons-store";
import { addonEnabled } from "@/lib/addons/types";
import { executeAddon } from "@/lib/addons/platform";
import type { AddonComposerBinding } from "@/lib/addons/use-addon-composer-adapter";
import type { SlashCommandItem } from "@/lib/agent-chat/slash-commands";
import { AddonView } from "./addon-view";
/** Composer actions for the attach menu's "Add-ons" group. A row that cannot
 *  run stays visible, dimmed, with the specific reason next to its add-on. */
export function useAddonComposerActions({
  id: composerId,
  registered,
  unavailable,
}: AddonComposerBinding) {
  const installed = useAddonsStore((s) => s.installed);
  const paused = useAddonsStore((s) => s.paused);
  return useMemo<SlashCommandItem[]>(
    () =>
      paused
        ? []
        : installed.filter(addonEnabled).flatMap(({ manifest }) =>
            manifest.contributes.composerActions.map((action) => ({
              id: `addon:${manifest.id}:${action.id}`,
              label: action.title,
              // The reason leads so a narrow menu truncates the name, not it.
              description: registered
                ? manifest.name
                : `${unavailable ?? "Connecting to this draft…"} · ${manifest.name}`,
              command: "",
              icon: Puzzle,
              group: "Add-ons",
              disabled: !registered,
              onSelect: () => {
                void executeAddon(
                  manifest.id,
                  action.id,
                  "composerActions",
                  composerId,
                );
              },
            })),
          ),
    [installed, paused, composerId, registered, unavailable],
  );
}
/** The one expandable add-on area above the composer footer. Renders nothing
 *  (no wrapper, no spacing) unless an add-on opened it for this composer. */
export function ComposerAddonAccessory({
  composerId,
  onClose,
}: {
  composerId: string;
  /** Called after the user closes it, so focus can return to the draft. */
  onClose?: () => void;
}) {
  const accessory = useAddonsStore((s) =>
    composerId && s.accessory?.composerId === composerId ? s.accessory : null,
  );
  const installed = useAddonsStore((s) => s.installed);
  const paused = useAddonsStore((s) => s.paused);
  if (!accessory || paused) return null;
  const plugin = installed.find((i) => i.manifest.id === accessory.pluginId);
  const declaration = plugin?.manifest.contributes.composerViews.find(
    (v) => v.id === accessory.view,
  );
  if (!plugin || !addonEnabled(plugin) || !declaration) return null;
  const label = `${declaration.title} — ${plugin.manifest.name}`;
  return (
    <section
      aria-label={label}
      data-testid="composer-addon-accessory"
      className="mx-3 my-2 max-h-72 overflow-auto rounded-lg border bg-background"
    >
      <div className="flex items-center justify-between gap-2 border-b py-1 pl-3 pr-1 text-label">
        <span className="min-w-0 truncate">
          {declaration.title}{" "}
          <span className="text-muted-foreground">
            · {plugin.manifest.name}
          </span>
        </span>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          aria-label="Close add-on accessory"
          onClick={() => {
            useAddonsStore.setState({ accessory: null });
            onClose?.();
          }}
        >
          <X />
        </Button>
      </div>
      <AddonView
        id={accessory.pluginId}
        view={accessory.view}
        workspaceId={accessory.workspaceId}
        composerId={composerId}
        kind="composerViews"
        label={label}
        region={false}
      />
    </section>
  );
}
