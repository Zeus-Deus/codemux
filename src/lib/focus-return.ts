import { selectActiveWorkspaceId, useAppStore } from "@/stores/app-store";

/**
 * Giving focus back after an overlay closes.
 *
 * Store-driven overlays (the command palette, Settings) have no trigger for
 * Radix to return to, so focus used to fall to `<body>` and the next
 * keystroke went nowhere. These helpers put it back where it came from, or
 * in the composer the user was working in.
 */

/** The element that has focus, or null when nothing does (`<body>`). */
export function currentFocus(): HTMLElement | null {
  if (typeof document === "undefined") return null;
  const el = document.activeElement;
  return el instanceof HTMLElement && el !== document.body ? el : null;
}

/** True when focus has fallen out of the page's controls. */
export function focusIsLost(): boolean {
  return currentFocus() === null;
}

function isShown(el: HTMLElement): boolean {
  return el.getClientRects().length > 0 && !el.closest("[inert]");
}

/** The message field of the pane the user is working in: the active pane's
 *  composer, else the first one on screen (the new-chat landing has none
 *  inside a pane). */
export function activeComposerInput(): HTMLElement | null {
  if (typeof document === "undefined") return null;
  const inputs = [...document.querySelectorAll<HTMLElement>("[data-composer-input]")].filter(isShown);
  if (inputs.length <= 1) return inputs[0] ?? null;
  const state = useAppStore.getState();
  const workspaceId = selectActiveWorkspaceId(state);
  const workspace = state.appState?.workspaces.find((w) => w.workspace_id === workspaceId);
  const surface = workspace?.surfaces.find((s) => s.surface_id === workspace.active_surface_id);
  const paneId = surface?.active_pane_id;
  const inActivePane = paneId
    ? inputs.find((input) => input.closest<HTMLElement>("[data-pane-drop-id]")?.dataset.paneDropId === paneId)
    : undefined;
  return inActivePane ?? inputs[0];
}

/** On touch screens, focusing a text field raises the on-screen keyboard,
 *  so focus is only ever handed back there, never moved into a field. */
export function isTouchScreen(): boolean {
  return typeof window !== "undefined" && (window.matchMedia?.("(pointer: coarse)").matches ?? false);
}

/** The composer to land in when nothing better is left, if any. */
export function composerFallback(): HTMLElement | null {
  return isTouchScreen() ? null : activeComposerInput();
}

function tryFocus(el: HTMLElement | null): boolean {
  if (!el?.isConnected) return false;
  el.focus({ preventScroll: true });
  return document.activeElement === el;
}

/**
 * Focus `origin` if it is still on the page and can take focus, otherwise the
 * active composer (not on touch screens). Returns whether anything took focus.
 */
export function restoreFocus(origin: HTMLElement | null): boolean {
  return tryFocus(origin) || tryFocus(composerFallback());
}

/**
 * Focus the first of `find`'s candidates that takes focus once it renders, checking each frame for about a
 * second (Settings and the shell behind it load lazily). Stands down as soon
 * as anything else has focus, so a click or a keystroke in the meantime wins.
 */
export function focusWhenRendered(
  find: () => ReadonlyArray<HTMLElement | null>,
  frames = 60,
): void {
  if (typeof requestAnimationFrame === "undefined") return;
  const step = (left: number) => {
    if (!focusIsLost()) return;
    if (find().some(tryFocus)) return;
    if (left > 0) requestAnimationFrame(() => step(left - 1));
  };
  requestAnimationFrame(() => step(frames));
}
