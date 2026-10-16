/**
 * Area ids: the `area` of a contribution. Each comment says what the area
 * reads from the contribution (`data` or `render`) and the props `render`
 * receives. Full reference: docs/plugins/README.md.
 */

/** data: PaletteCommandData */
export const PALETTE_AREA = "palette.commands";
/** data: KeybindData. Rebindable in Settings → Shortcuts. */
export const KEYBINDS_AREA = "keybinds";
/** title, data: { icon? }, render({ workspaceId }). Opened from the right panel "+" menu or host.openRightPane(id). */
export const RIGHT_PANEL_AREA = "rightPanel.panes";
/** title, render(). A full page; opened with host.openPage(id). */
export const PAGES_AREA = "pages";
/** Via ctx.registerSettingsPage({ title, render }). */
export const SETTINGS_PAGES_AREA = "settings.pages";
/** render(). Appended to the end of Settings → Appearance. */
export const APPEARANCE_EXTRA_AREA = "appearance.extra";
/** data: ComposerActionData. Rows in the composer "+" menu. */
export const COMPOSER_ACTIONS_AREA = "composer.actions";
/** render({ workspaceId, threadId }). Above the composer footer. */
export const COMPOSER_ACCESSORY_AREA = "composer.accessory";
/** render({ workspaceId, threadId }). Below the composer card. */
export const COMPOSER_FOOTER_AREA = "composer.footer";
/** render(). Title bar action island. Stop pointerdown propagation on controls. */
export const TITLEBAR_ACTIONS_AREA = "titlebar.actions";
/** render(). Between the workspace list and the sidebar footer. */
export const SIDEBAR_SECTION_AREA = "sidebar.section";
/** render(). The first enabled contribution replaces the empty-sidebar plate. */
export const SIDEBAR_EMPTY_STATE_AREA = "sidebar.emptyState";
/** render(LandingBackdropProps). Behind the new-thread landing, pointer-events: none. */
export const LANDING_BACKDROP_AREA = "landing.backdrop";
/** render({ workspaceId }). Below the composer on the new-thread landing. */
export const LANDING_BELOW_COMPOSER_AREA = "landing.belowComposer";
/** data: ToolRendererData. The first matching renderer wins over the core card. */
export const TOOL_RENDERER_AREA = "transcript.toolRenderer";
/** data: ThemeDefinition. Listed in the theme picker; never auto-selected. */
export const THEMES_AREA = "themes";
/** render(). Floating UI or headless components above the whole app. */
export const APP_OVERLAY_AREA = "app.overlay";
