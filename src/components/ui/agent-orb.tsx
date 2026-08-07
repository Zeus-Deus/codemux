import { ThinkingOrb, type OrbSize } from "thinking-orbs";

import {
  ORB_FALLBACK_STATE,
  resolveOrbState,
  type OrbActivity,
} from "@/lib/orb-state";
import {
  selectOrbMatchActivity,
  useSettingsStore,
} from "@/stores/settings-store";

interface Props extends OrbActivity {
  /**
   * Two tuned presets ship, and they are separate designs rather than one
   * scaled drawing: 20 for inline rows, 64 for avatar-scale surfaces.
   */
  size?: OrbSize;
  /**
   * Accessible name. Defaults to the library's per-state label ("Searching…",
   * "Connecting…"), which is usually what a live row wants. Pass a stable
   * string where the surrounding row already announces its own status, so
   * the two don't read as a contradiction.
   */
  "aria-label"?: string;
  /**
   * Hide from assistive tech. Use where the surrounding row already carries
   * a `role="status"` — two live regions describing one thing read as a
   * contradiction.
   */
  "aria-hidden"?: boolean;
  className?: string;
}

/**
 * The agent activity indicator — the one animated mark shown wherever an
 * agent is currently moving: sidebar workspace cards, the in-flight turn in
 * a thread, running subagent rows, and the docked composer strip.
 *
 * Always monochrome. The orb inks white on dark and black on light from the
 * library's own palette (`theme="auto"` reads the `dark`/`light` root class
 * this app already sets), so it never competes with the accent and never
 * needs a color token. Red stays reserved for workspaces that need a human.
 *
 * Two things are centralized here rather than at each call site: the
 * activity → state mapping (`src/lib/orb-state.ts`) and the Settings →
 * Appearance → Agents "Match the orb to the activity" pin. With the toggle
 * off, every orb in the app paints {@link ORB_FALLBACK_STATE} regardless of
 * what the caller passed.
 */
export function AgentOrb({
  size = 20,
  "aria-label": ariaLabel,
  "aria-hidden": ariaHidden,
  className,
  ...activity
}: Props) {
  const matchActivity = useSettingsStore(selectOrbMatchActivity);
  const state = matchActivity ? resolveOrbState(activity) : ORB_FALLBACK_STATE;

  return (
    <ThinkingOrb
      state={state}
      size={size}
      theme="auto"
      aria-label={ariaLabel}
      aria-hidden={ariaHidden}
      className={className}
      data-orb-state={state}
    />
  );
}
