import { useMemo } from "react";
import { Puzzle, X } from "lucide-react";
import { useAddonsStore } from "@/stores/addons-store";
import { addonEnabled } from "@/lib/addons/types";
import { executeAddon } from "@/lib/addons/platform";
import type { SlashCommandItem } from "@/lib/agent-chat/slash-commands";
import { AddonView } from "./addon-view";
export function useAddonComposerActions(
  composerId: string,
  registered: boolean,
) {
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
              description: registered
                ? manifest.name
                : "Open a local workspace with an available draft",
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
    [installed, paused, composerId, registered],
  );
}
export function ComposerAddonAccessory({ composerId }: { composerId: string }) {
  const accessory = useAddonsStore((s) =>
    s.accessory?.composerId === composerId ? s.accessory : null,
  );
  const installed = useAddonsStore((s) => s.installed);
  if (!accessory) return null;
  const plugin = installed.find((i) => i.manifest.id === accessory.pluginId);
  const declaration = plugin?.manifest.contributes.composerViews.find(
    (v) => v.id === accessory.view,
  );
  if (!plugin || !declaration) return null;
  return (
    <section
      aria-label={`${declaration.title} — ${plugin.manifest.name}`}
      className="mx-3 my-2 max-h-72 overflow-auto rounded-lg border bg-background"
    >
      <div className="flex items-center justify-between border-b px-3 py-2 text-xs">
        <span>
          {declaration.title}{" "}
          <span className="text-muted-foreground">
            · {plugin.manifest.name}
          </span>
        </span>
        <button
          type="button"
          aria-label="Close add-on accessory"
          onClick={() => useAddonsStore.setState({ accessory: null })}
        >
          <X className="size-4" />
        </button>
      </div>
      <AddonView
        id={accessory.pluginId}
        view={accessory.view}
        workspaceId={accessory.workspaceId}
        composerId={composerId}
        kind="composerViews"
      />
    </section>
  );
}
