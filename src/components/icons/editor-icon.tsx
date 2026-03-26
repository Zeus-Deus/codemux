import { Code2 } from "lucide-react";
import { cn } from "@/lib/utils";

import vscodeIcon from "@/assets/editor-icons/vscode.svg";
import vscodiumIcon from "@/assets/editor-icons/vscodium.svg";
import cursorIcon from "@/assets/editor-icons/cursor.png";
import zedIcon from "@/assets/editor-icons/zed.png";
import intellijIcon from "@/assets/editor-icons/intellij.svg";
import golandIcon from "@/assets/editor-icons/goland.svg";
import webstormIcon from "@/assets/editor-icons/webstorm.svg";
import sublimeIcon from "@/assets/editor-icons/sublime.svg";

interface EditorIconProps {
  id: string;
  className?: string;
}

const ICON_MAP: Record<string, string> = {
  code: vscodeIcon,
  codium: vscodiumIcon,
  cursor: cursorIcon,
  zed: zedIcon,
  idea: intellijIcon,
  goland: golandIcon,
  webstorm: webstormIcon,
  sublime_text: sublimeIcon,
};

export function EditorIcon({ id, className }: EditorIconProps) {
  const icon = ICON_MAP[id];
  if (icon) {
    return (
      <img
        src={icon}
        alt=""
        className={cn("shrink-0 object-contain", className)}
      />
    );
  }
  return <Code2 className={cn("shrink-0", className)} />;
}
