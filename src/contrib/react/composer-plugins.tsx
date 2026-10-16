/**
 * Composer integration for plugins: registers each mounted composer for
 * `host.composer`, turns `composer.actions` into "+" menu rows, and mounts
 * the accessory/footer slots. With no plugin enabled the rows are [] and the
 * slots render nothing.
 */
import { useEffect, useId, useLayoutEffect, useMemo, useRef, type RefObject } from "react";
import { Puzzle } from "lucide-react";
import type { SlashCommandItem } from "@/lib/agent-chat/slash-commands";
import { COMPOSER_ACTIONS_AREA } from "@/sdk/areas";
import type { ComposerActionData } from "@/sdk/host";
import { host } from "@/sdk/host";
import { reportPluginError } from "../activation";
import { pluginIdOf, pluginLabel } from "../attribution";
import { appendText, markComposerFocused, registerComposer } from "../composer-registry";
import { useContributions } from "./use-contributions";

export interface PluginComposerBinding {
  id: string;
  workspaceId: string | null;
  threadId: string | null;
  /** Call from the composer's focus handler. */
  markFocused: () => void;
}

export function usePluginComposerBinding(
  workspaceId: string | null,
  threadId: string | null,
  draft: string,
  onDraftChange: (text: string) => void,
  textareaRef: RefObject<HTMLTextAreaElement | null>,
): PluginComposerBinding {
  const id = useId();
  const latest = useRef({ draft, onDraftChange });
  useLayoutEffect(() => {
    latest.current = { draft, onDraftChange };
  }, [draft, onDraftChange]);
  useEffect(
    () =>
      registerComposer(id, {
        workspaceId,
        threadId,
        getDraft: () => latest.current.draft,
        insertText: (text) => {
          const next = appendText(latest.current.draft, text);
          latest.current.draft = next;
          latest.current.onDraftChange(next);
        },
        focus: () => textareaRef.current?.focus(),
      }),
    [id, workspaceId, threadId, textareaRef],
  );
  return useMemo(
    () => ({ id, workspaceId, threadId, markFocused: () => markComposerFocused(id) }),
    [id, workspaceId, threadId],
  );
}

export function usePluginComposerActions(binding: PluginComposerBinding): SlashCommandItem[] {
  const items = useContributions(COMPOSER_ACTIONS_AREA);
  return useMemo(() => {
    if (items.length === 0) return [];
    return items.flatMap((c) => {
      const data = c.data as ComposerActionData | undefined;
      if (!data || typeof data.label !== "string" || typeof data.run !== "function") return [];
      const name = pluginLabel(pluginIdOf(c.id));
      return [
        {
          id: `plugin:${c.id}`,
          label: data.label,
          description: data.description ? `${data.description} · ${name}` : name,
          command: "",
          icon: (data.icon ?? Puzzle) as SlashCommandItem["icon"],
          group: "Plugins",
          onSelect: () => {
            binding.markFocused();
            try {
              data.run({ workspaceId: binding.workspaceId, threadId: binding.threadId, composer: host.composer });
            } catch (error) {
              reportPluginError(name, "composer action failed", error);
            }
          },
        } satisfies SlashCommandItem,
      ];
    });
  }, [items, binding]);
}
