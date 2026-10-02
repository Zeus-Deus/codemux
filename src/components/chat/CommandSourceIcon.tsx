import type { SlashCommandItem } from "@/lib/agent-chat/slash-commands";
import { ProviderLogo } from "./provider-logo";
import codemuxMark from "@/assets/codemux-logomark.svg";

/** One source mark; the accessible label and selected-row detail identify the type. */
export function CommandSourceIcon({ identity }: {
  identity: NonNullable<SlashCommandItem["identity"]>;
}) {
  return (
    <span
      role="img"
      aria-label={identity.label}
      title={identity.label}
      data-command-kind={identity.kind}
      className="flex w-3.5 shrink-0 items-center"
    >
      {identity.provider === "codemux" ? (
        <img src={codemuxMark} alt="" className="size-3.5 shrink-0 invert dark:invert-0" />
      ) : (
        <ProviderLogo provider={identity.provider} className="size-3.5" />
      )}
    </span>
  );
}
