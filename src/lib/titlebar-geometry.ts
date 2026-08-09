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
 * Horizontal room the OS window buttons need at the top-right.
 *
 * Desktop only: the web remote client renders no window controls, so it
 * reserves nothing (see {@link panelClusterRight}).
 */
export const WINDOW_CONTROLS_RESERVE = 104;

/** Width of one titlebar-sized `PaneActionButton` in the fixed cluster. */
export const PANEL_CONTROL_WIDTH = 28;

/**
 * Reserved width for the open panel's two 28px controls. This includes its
 * internal 2px gap and the existing 6px breathing room around the cluster.
 */
export const PANEL_CLUSTER_WIDTH = 64;

/** Gap between the cluster and whatever sits beside it. */
export const PANEL_CLUSTER_GAP = 6;

/** Distance from the window's right edge to the panel-control cluster. */
export function panelClusterRight(remoteClient: boolean): number {
  return remoteClient ? PANEL_CLUSTER_GAP : WINDOW_CONTROLS_RESERVE;
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
  panelOpen: boolean,
): number {
  const clusterWidth = panelOpen ? PANEL_CLUSTER_WIDTH : PANEL_CONTROL_WIDTH;
  return panelClusterRight(remoteClient) + clusterWidth + PANEL_CLUSTER_GAP;
}
