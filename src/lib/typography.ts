import type { AppearanceSettings } from "@/tauri/types";

export type TypographyMode = "simple" | "advanced";

export const TYPOGRAPHY_DEFAULTS = {
  mode: "simple" as TypographyMode,
  interfaceFamily: null as string | null,
  interfaceSize: 16,
  conversationFamily: null as string | null,
  conversationSize: 14,
  codeFamily: null as string | null,
  codeSize: 13,
  terminalFamily: null as string | null,
  terminalSize: 13,
} as const;

export const TYPOGRAPHY_RANGES = {
  interface: { min: 13, max: 19 },
  conversation: { min: 12, max: 20 },
  // Code shares the terminal ceiling: a pre-migration blob adopts its terminal
  // size as the code size, and that must not shrink a 20–22px choice.
  code: { min: 10, max: 22 },
  terminal: { min: 10, max: 22 },
} as const;

export const DEFAULT_INTERFACE_FONT_STACK =
  "'DM Sans Variable', 'DM Sans', ui-sans-serif, system-ui, sans-serif";
export const DEFAULT_CODE_FONT_STACK =
  "'JetBrains Mono Variable', 'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace";

const GENERIC_FAMILIES = new Set([
  "serif",
  "sans-serif",
  "monospace",
  "system-ui",
  "ui-serif",
  "ui-sans-serif",
  "ui-monospace",
  "cursive",
  "fantasy",
]);

export interface ResolvedTypography {
  mode: TypographyMode;
  interfacePreference: string | null;
  conversationPreference: string | null;
  codePreference: string | null;
  terminalPreference: string | null;
  interfaceFamily: string;
  conversationFamily: string;
  codeFamily: string;
  terminalFamily: string;
  interfaceSize: number;
  conversationSize: number;
  codeSize: number;
  terminalSize: number;
}

function finiteNumber(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

export function clampTypographySize(
  value: unknown,
  range: { readonly min: number; readonly max: number },
  fallback: number,
): number {
  return Math.min(range.max, Math.max(range.min, Math.round(finiteNumber(value, fallback))));
}

/**
 * Preferences store one family, never executable CSS. Synced values,
 * hand-edited settings, and the picker share this bounded path before any of
 * them reaches a style.
 */
export function normalizeFontFamily(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const normalized = value.replace(/[\u0000-\u001f\u007f]/g, "").trim().slice(0, 120);
  return normalized.length > 0 ? normalized : null;
}

export function quoteFontFamily(family: string): string {
  if (GENERIC_FAMILIES.has(family.toLowerCase())) return family.toLowerCase();
  return `"${family.replace(/\\/g, "").replace(/"/g, "")}"`;
}

export function fontStack(preference: string | null, fallback: string): string {
  return preference ? `${quoteFontFamily(preference)}, ${fallback}` : fallback;
}

export function resolveTypographySettings(
  appearance: Partial<AppearanceSettings> | null | undefined,
): ResolvedTypography {
  const mode: TypographyMode =
    appearance?.typography_mode === "advanced" ? "advanced" : TYPOGRAPHY_DEFAULTS.mode;

  const interfacePreference = normalizeFontFamily(appearance?.interface_font_family);
  const storedConversationPreference = normalizeFontFamily(appearance?.conversation_font_family);
  // `shell_font` was the original terminal-family preference. It stands in as
  // the shared developer face until a write in the richer UI clears it.
  const codePreference = normalizeFontFamily(
    appearance?.code_font_family ?? appearance?.shell_font,
  );
  const storedTerminalPreference = normalizeFontFamily(
    appearance?.terminal_font_family ?? appearance?.shell_font,
  );

  const interfaceSize = clampTypographySize(
    appearance?.interface_font_size,
    TYPOGRAPHY_RANGES.interface,
    TYPOGRAPHY_DEFAULTS.interfaceSize,
  );
  const storedConversationSize = clampTypographySize(
    appearance?.conversation_font_size,
    TYPOGRAPHY_RANGES.conversation,
    TYPOGRAPHY_DEFAULTS.conversationSize,
  );
  const storedTerminalSize = clampTypographySize(
    appearance?.terminal_font_size,
    TYPOGRAPHY_RANGES.terminal,
    TYPOGRAPHY_DEFAULTS.terminalSize,
  );
  const codeSize = clampTypographySize(
    appearance?.code_font_size,
    TYPOGRAPHY_RANGES.code,
    // Older settings only had terminal_font_size, and Rust preserves a missing
    // code size as null. The settings UI materializes it before Advanced mode
    // presents the two sizes as independent.
    storedTerminalSize,
  );

  // Simple mode has two decisions: interface and developer. Conversation
  // follows the interface family at a quieter -2px rhythm; terminal follows
  // the developer font exactly. Advanced mode reveals the stored per-surface
  // overrides without destroying them when users switch.
  const conversationPreference =
    mode === "advanced" ? storedConversationPreference ?? interfacePreference : interfacePreference;
  const terminalPreference =
    mode === "advanced" ? storedTerminalPreference ?? codePreference : codePreference;
  const conversationSize =
    mode === "advanced"
      ? storedConversationSize
      : clampTypographySize(
          interfaceSize - 2,
          TYPOGRAPHY_RANGES.conversation,
          TYPOGRAPHY_DEFAULTS.conversationSize,
        );
  const terminalSize = mode === "advanced" ? storedTerminalSize : codeSize;

  const interfaceFamily = fontStack(interfacePreference, DEFAULT_INTERFACE_FONT_STACK);
  const codeFamily = fontStack(codePreference, DEFAULT_CODE_FONT_STACK);

  return {
    mode,
    interfacePreference,
    conversationPreference,
    codePreference,
    terminalPreference,
    interfaceFamily,
    conversationFamily: conversationPreference
      ? fontStack(conversationPreference, DEFAULT_INTERFACE_FONT_STACK)
      : interfaceFamily,
    codeFamily,
    terminalFamily: terminalPreference
      ? fontStack(terminalPreference, DEFAULT_CODE_FONT_STACK)
      : codeFamily,
    interfaceSize,
    conversationSize,
    codeSize,
    terminalSize,
  };
}

/**
 * The machine-local `terminal.font_family` override predates synced
 * typography. It only wins while the synced blob carries no terminal family of
 * its own, so any choice in the settings UI retires it.
 */
export function resolveTerminalFontFamily(
  typography: ResolvedTypography,
  legacyLocalFamily: string | null | undefined,
): string {
  if (typography.terminalPreference || !legacyLocalFamily) return typography.terminalFamily;
  return legacyLocalFamily;
}

/** Apply the resolved contract once; every text renderer consumes these tokens. */
export function applyTypography(root: HTMLElement, typography: ResolvedTypography): void {
  // At the default size the root stays untouched so browser-level font scaling
  // keeps working; only an explicit interface size pins it to pixels.
  if (typography.interfaceSize === TYPOGRAPHY_DEFAULTS.interfaceSize) {
    root.style.removeProperty("font-size");
  } else {
    root.style.fontSize = `${typography.interfaceSize}px`;
  }
  root.style.setProperty("--font-interface", typography.interfaceFamily);
  root.style.setProperty("--font-conversation", typography.conversationFamily);
  root.style.setProperty("--font-code", typography.codeFamily);
  root.style.setProperty("--font-terminal", typography.terminalFamily);
  root.style.setProperty("--font-size-conversation", `${typography.conversationSize}px`);
  root.style.setProperty("--font-size-code", `${typography.codeSize}px`);
  root.style.setProperty("--font-size-terminal", `${typography.terminalSize}px`);
  root.style.setProperty("--line-height-code", `${Math.round(typography.codeSize * 1.5)}px`);
}
