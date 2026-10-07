import { useEffect, useMemo, useState } from "react";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";
import { parseKeyCombo } from "@/lib/keybind-utils";
import { DEFAULT_JUMP_MODIFIER } from "./sidebar-inbox-jump";

/**
 * Whether the workspace-jump modifier is physically held, so the sidebar can
 * show each jump target's digit. Shared by the expanded inbox and the
 * collapsed rail so the two always reveal their badges on the same key.
 */
export function useJumpHintsVisible(): boolean {
  // Respect the user's actual resolved binding for slot 1 (so a rebind to
  // Ctrl/Alt tracks the right key); fall back to the default modifier. A rebind
  // to a non-Alt/Ctrl chord (e.g. Shift-only) simply shows no held-modifier
  // hints.
  const { keybindMap } = useResolvedKeybinds();
  const jumpModifierKey = useMemo(() => {
    const keys =
      keybindMap.get("workspaceJump1")?.activeKeys ??
      `${DEFAULT_JUMP_MODIFIER}+1`;
    const parsed = parseKeyCombo(keys);
    if (parsed.ctrl) return "Control";
    if (parsed.alt) return "Alt";
    return null;
  }, [keybindMap]);

  // Clear on keyup, blur, and visibilitychange so the hints can never get
  // stuck open.
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    if (!jumpModifierKey) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === jumpModifierKey) setVisible(true);
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === jumpModifierKey) setVisible(false);
    };
    const clear = () => setVisible(false);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", clear);
    document.addEventListener("visibilitychange", clear);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("blur", clear);
      document.removeEventListener("visibilitychange", clear);
      setVisible(false);
    };
  }, [jumpModifierKey]);

  return visible;
}
