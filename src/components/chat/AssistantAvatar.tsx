import { Sparkle } from "lucide-react";

import { cn } from "@/lib/utils";
import type { AgentChatProviderKind } from "@/tauri/types";

import { ProviderLogo } from "./provider-logo";

interface Props {
  /** The session's chat provider. When set, the avatar shows that
   *  provider's official branded mark via
   *  {@link ProviderLogo}; when absent it falls back to the generic ember
   *  sparkle. */
  provider?: AgentChatProviderKind | null;
}

/**
 * The tinted square shown once at the head of each assistant turn
 * (design D4). Sits in a 29px gutter column; subsequent rows of the same
 * turn render an empty gutter so their content stays aligned under the
 * first row.
 *
 * With a provider it renders that provider's official mark; the Claude
 * mark is ember-toned so it keeps the ember wash, while the neutral
 * Codex / Cursor / Grok / OpenCode marks sit on a subtle neutral wash
 * instead (a hard-coded ember tint reads wrong behind them). Without a
 * provider it keeps the generic ember sparkle fallback.
 */
export function AssistantAvatar({ provider }: Props) {
  if (provider) {
    return (
      <span
        aria-hidden
        data-provider={provider}
        // The wash is stated as intent, not inferred from a class string:
        // tests assert which wash a mark gets, the ladder owns the value.
        data-wash={provider === "claude" ? "ember" : "neutral"}
        className={cn(
          "flex h-[29px] w-[29px] shrink-0 items-center justify-center rounded-md",
          // Match the mark: ember wash for the ember-toned Claude mark,
          // a subtle neutral wash for the other provider marks.
          // Both token-based so they track the theme.
          provider === "claude" ? "bg-accent-ember/15" : "bg-surface-3",
        )}
      >
        <ProviderLogo provider={provider} className="h-[15px] w-[15px]" />
      </span>
    );
  }

  return (
    <span
      aria-hidden
      data-wash="ember"
      className="flex h-[29px] w-[29px] shrink-0 items-center justify-center rounded-md bg-accent-ember/15 text-accent-ember"
    >
      <Sparkle className="h-[15px] w-[15px]" strokeWidth={1.4} />
    </span>
  );
}
