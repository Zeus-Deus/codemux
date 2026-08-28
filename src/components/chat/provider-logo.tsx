import { cn } from "@/lib/utils";
import type { AgentChatProviderKind } from "@/tauri/types";

// Reuse the branded marks Codemux already ships for the preset bar
// (`src/components/icons/preset-icon.tsx`). Vite's asset-URL import
// hands us a resolved path string — we render it via `<img>` so the
// SVG is treated as a self-contained asset rather than inlined markup.
import claudeIcon from "@/assets/preset-icons/claude.svg";
import codexIcon from "@/assets/preset-icons/codex.svg";
import cursorIcon from "@/assets/preset-icons/cursor-agent.svg";
import hermesIcon from "@/assets/preset-icons/hermes.svg";
import opencodeIcon from "@/assets/preset-icons/opencode.svg";

const PROVIDER_ICON_MAP: Record<AgentChatProviderKind, string> = {
  claude: claudeIcon,
  codex: codexIcon,
  cursor: cursorIcon,
  opencode: opencodeIcon,
  hermes: hermesIcon,
};

const PROVIDER_LABEL: Record<AgentChatProviderKind, string> = {
  claude: "Claude",
  codex: "Codex",
  cursor: "Cursor",
  opencode: "OpenCode",
  hermes: "Hermes",
};

interface Props {
  provider: AgentChatProviderKind;
  className?: string;
}

export function ProviderLogo({ provider, className }: Props) {
  return (
    <img
      src={PROVIDER_ICON_MAP[provider]}
      alt={PROVIDER_LABEL[provider]}
      data-provider={provider}
      className={cn("shrink-0 object-contain", className)}
    />
  );
}
