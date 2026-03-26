import { Terminal, TerminalSquare } from "lucide-react";
import { cn } from "@/lib/utils";

import claudeIcon from "@/assets/preset-icons/claude.svg";
import codexIcon from "@/assets/preset-icons/codex.svg";
import opencodeIcon from "@/assets/preset-icons/opencode.svg";
import geminiIcon from "@/assets/preset-icons/gemini.svg";
import copilotIcon from "@/assets/preset-icons/copilot.svg";
import mastracodeIcon from "@/assets/preset-icons/mastracode.svg";
import piIcon from "@/assets/preset-icons/pi.svg";
import cursorAgentIcon from "@/assets/preset-icons/cursor-agent.svg";

interface PresetIconProps {
  icon: string | null;
  className?: string;
}

const ICON_MAP: Record<string, string> = {
  claude: claudeIcon,
  codex: codexIcon,
  opencode: opencodeIcon,
  gemini: geminiIcon,
  copilot: copilotIcon,
  mastracode: mastracodeIcon,
  pi: piIcon,
  "cursor-agent": cursorAgentIcon,
};

export function PresetIcon({ icon, className }: PresetIconProps) {
  if (!icon) {
    return <TerminalSquare className={cn("shrink-0", className)} />;
  }

  if (icon === "terminal") {
    return <Terminal className={cn("shrink-0", className)} />;
  }

  const src = ICON_MAP[icon];
  if (src) {
    return (
      <img
        src={src}
        alt=""
        className={cn("shrink-0 object-contain", className)}
      />
    );
  }

  return <TerminalSquare className={cn("shrink-0", className)} />;
}
