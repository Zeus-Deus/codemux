import { BotMessageSquare, Terminal, TerminalSquare } from "lucide-react";
import { cn } from "@/lib/utils";

import claudeIcon from "@/assets/preset-icons/claude.svg";
import codexIcon from "@/assets/preset-icons/codex.svg";
import opencodeIcon from "@/assets/preset-icons/opencode.svg";
import geminiIcon from "@/assets/preset-icons/gemini.svg";
import copilotIcon from "@/assets/preset-icons/copilot.svg";
import mastracodeIcon from "@/assets/preset-icons/mastracode.svg";
import piIcon from "@/assets/preset-icons/pi.svg";
import cursorAgentIcon from "@/assets/preset-icons/cursor-agent.svg";
import antigravityIcon from "@/assets/preset-icons/antigravity.svg";
import ampIcon from "@/assets/preset-icons/amp.svg";
import grokIcon from "@/assets/preset-icons/grok.svg";
import factoryIcon from "@/assets/preset-icons/factory.svg";

interface PresetIconProps {
  icon: string | null;
  className?: string;
}

/**
 * Brand marks that ship as a bare white silhouette on transparency.
 *
 * Nothing in those files carries a second colour, so on a light canvas they
 * paint white on white and the control simply is not there. An `<img>`-loaded
 * SVG cannot inherit `currentColor`, so the scheme-correct reading of "white
 * mark" is the inverted one: `invert(1)` leaves alpha alone and turns the
 * silhouette near-black, which is the light-canvas artwork each of these
 * brands publishes anyway. Marks that carry their own plate or brand hue
 * (Claude, Grok, Gemini, …) are deliberately absent — inverting those would
 * misrepresent the brand rather than rescue it.
 */
const WHITE_ONLY_MARKS = new Set(["codex", "copilot", "mastracode", "pi"]);

/** Tailwind classes that flip a {@link WHITE_ONLY_MARKS} mark for the scheme. */
export function whiteMarkClass(icon: string): string | undefined {
  return WHITE_ONLY_MARKS.has(icon) ? "invert dark:invert-0" : undefined;
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
  antigravity: antigravityIcon,
  amp: ampIcon,
  grok: grokIcon,
  factory: factoryIcon,
};

export function PresetIcon({ icon, className }: PresetIconProps) {
  if (!icon) {
    return <TerminalSquare className={cn("shrink-0", className)} />;
  }

  if (icon === "terminal") {
    return <Terminal className={cn("shrink-0", className)} />;
  }

  // The Chat Agent builtin renders a lucide glyph inline so it stays
  // crisp at any size and picks up the current foreground color (which
  // raster SVG assets don't always do reliably across themes).
  if (icon === "chat-agent") {
    return <BotMessageSquare className={cn("shrink-0", className)} />;
  }

  const src = ICON_MAP[icon];
  if (src) {
    return (
      <img
        src={src}
        alt=""
        className={cn("shrink-0 object-contain", whiteMarkClass(icon), className)}
      />
    );
  }

  return <TerminalSquare className={cn("shrink-0", className)} />;
}
