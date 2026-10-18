/**
 * The one place the floating titlebar band's geometry is written down.
 *
 * The band is 40px of window chrome that no surface owns outright: the
 * sidebar toggle, the workspace island, the right panel's own tab row and
 * the native window buttons all live inside it. Two of those participants
 * are in different React trees (`title-bar.tsx` renders an overlay outside
 * the layout; `right-panel.tsx` renders inside it), so the numbers they
 * have to agree on live here instead of being spelled twice.
 *
 * The rule that shapes all of it: **the top-right cluster is fixed**. The
 * panel-level controls sit at the window's top-right corner and stay there
 * whether the panel is open, closed, narrow or wide. Everything else — the
 * workspace island's right edge, the panel tab row's right padding — is
 * derived from that fixed corner, never the other way round. Only the
 * cluster's occupied width changes: a closed panel has no expand button, so
 * the workspace band must not reserve an empty slot for one. The band used
 * to work the opposite way (the action island tracked the panel's left
 * edge, so opening the panel teleported the panel toggle across the
 * window and left a dead 40px strip above the panel's tabs).
 */

/** Height of the floating band — the `h-10` overlay in `title-bar.tsx`. */
export const TITLEBAR_BAND_HEIGHT = 40;

/**
 * How far the right panel's resize handle reaches left of the panel's edge
 * (its 1px seam plus 2px of grab area over the workspace). The titlebar's
 * drag layer stops this far short of the panel so it can't cover the handle.
 */
export const RIGHT_PANEL_RESIZER_REACH = 3;

/**
 * Horizontal room the OS window buttons need at the top-right.
 *
 * Desktop only: the web remote client renders no window controls, so it
 * reserves nothing (see {@link panelClusterRight}).
 */
export const WINDOW_CONTROLS_RESERVE = 104;

/**
 * Every px value below is at the default 16px root. The controls they
 * describe are rem-sized, so the interface-size setting scales them; callers
 * pass the live root size (`useRootFontPx`) and the reserves scale with it.
 */
const BASE_ROOT_PX = 16;

/** Width of one titlebar-sized control (`size-7`) in the fixed cluster. */
export const PANEL_CONTROL_WIDTH = 28;

/**
 * The right-hand run's rhythm: the gap between two 28px controls.
 *
 * The native window buttons are 32px hover boxes laid edge to edge, so their
 * glyphs sit 32px apart. Every 28px control to their left keeps that same
 * 32px glyph pitch — 4px between two of them, 2px between one and the first
 * window button — so the resource monitor, the details toggle, the panel
 * controls and the window buttons read as one evenly spaced row.
 */
export const TITLEBAR_RHYTHM_GAP = 4;

/** The window buttons' inset from the window's right edge (`right-1`). */
export const WINDOW_CONTROLS_INSET = 4;

/** The window buttons' footprint: three 32px buttons plus their inset. */
export const WINDOW_CONTROLS_WIDTH = 3 * 32 + WINDOW_CONTROLS_INSET;

/** Web client only: the cluster's inset from the bare window edge. */
export const PANEL_CLUSTER_GAP = 6;

export interface CornerClusterShape {
  /** The right panel is open, so its expand control is shown. */
  panelOpen: boolean;
  /** The resource monitor is enabled (it leads the cluster). */
  monitor?: boolean;
  /** The live root font size; defaults to 16. */
  remPx?: number;
}

/** Width of the fixed corner cluster: [monitor] details [expand] panel. */
export function cornerClusterWidth({
  panelOpen,
  monitor = false,
  remPx = BASE_ROOT_PX,
}: CornerClusterShape): number {
  const controls = 2 + (panelOpen ? 1 : 0) + (monitor ? 1 : 0);
  const base =
    controls * PANEL_CONTROL_WIDTH + (controls - 1) * TITLEBAR_RHYTHM_GAP;
  return (base * remPx) / BASE_ROOT_PX;
}

/** Distance from the window's right edge to the panel-control cluster. */
export function panelClusterRight(
  remoteClient: boolean,
  remPx: number = BASE_ROOT_PX,
): number {
  // 2px, not 4: a 28px control beside a 32px window button keeps the 32px
  // glyph pitch with half the gap (see TITLEBAR_RHYTHM_GAP).
  const base = remoteClient ? PANEL_CLUSTER_GAP : WINDOW_CONTROLS_WIDTH + 2;
  return (base * remPx) / BASE_ROOT_PX;
}

/**
 * Everything pinned to the top-right corner, as one number.
 *
 * Two consumers, one meaning — "content in the band must stop here":
 *   - the workspace island's `right`, based on the controls actually shown,
 *   - the right panel's tab-row padding when the panel owns the band
 *     (its tab row runs to the physical window edge, so it has to clear
 *     both the cluster and the window buttons above it).
 */
export function topRightReserve(
  remoteClient: boolean,
  shape: CornerClusterShape,
): number {
  const remPx = shape.remPx ?? BASE_ROOT_PX;
  return (
    panelClusterRight(remoteClient, remPx) +
    cornerClusterWidth(shape) +
    (TITLEBAR_RHYTHM_GAP * remPx) / BASE_ROOT_PX
  );
}
