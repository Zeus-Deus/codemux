import { describe, expect, it } from "vitest";

import {
  DEFAULT_CODE_FONT_STACK,
  DEFAULT_INTERFACE_FONT_STACK,
  applyTypography,
  fontStack,
  normalizeFontFamily,
  quoteFontFamily,
  resolveTypographySettings,
} from "./typography";

describe("typography settings", () => {
  it("resolves the simple two-font model and preserves the existing 16/14 rhythm", () => {
    const resolved = resolveTypographySettings({
      typography_mode: "simple",
      interface_font_family: "Atkinson Hyperlegible",
      interface_font_size: 16,
      code_font_family: "Fira Code",
      code_font_size: 13,
      conversation_font_family: "Ignored in simple mode",
      conversation_font_size: 20,
      terminal_font_family: "Ignored in simple mode",
      terminal_font_size: 22,
    });

    expect(resolved.conversationPreference).toBe("Atkinson Hyperlegible");
    expect(resolved.conversationSize).toBe(14);
    expect(resolved.terminalPreference).toBe("Fira Code");
    expect(resolved.terminalSize).toBe(13);
  });

  it("uses independent overrides in advanced mode and falls back by surface", () => {
    const resolved = resolveTypographySettings({
      typography_mode: "advanced",
      interface_font_family: "Aptos",
      interface_font_size: 17,
      conversation_font_family: null,
      conversation_font_size: 15,
      code_font_family: "Iosevka",
      code_font_size: 12,
      terminal_font_family: "Berkeley Mono",
      terminal_font_size: 14,
    });

    expect(resolved.conversationPreference).toBe("Aptos");
    expect(resolved.conversationSize).toBe(15);
    expect(resolved.terminalPreference).toBe("Berkeley Mono");
    expect(resolved.terminalSize).toBe(14);
  });

  it("falls back to legacy shell_font and clamps unsafe sizes", () => {
    const resolved = resolveTypographySettings({
      typography_mode: "advanced",
      shell_font: "Hack",
      interface_font_size: 999,
      conversation_font_size: Number.NaN,
      code_font_size: -4,
      terminal_font_size: 200,
    });

    expect(resolved.terminalPreference).toBe("Hack");
    expect(resolved.codePreference).toBe("Hack");
    expect(resolved.interfaceSize).toBe(19);
    expect(resolved.conversationSize).toBe(14);
    expect(resolved.codeSize).toBe(10);
    expect(resolved.terminalSize).toBe(22);
  });

  it("migrates the legacy shell font into the linked developer choice", () => {
    const resolved = resolveTypographySettings({
      typography_mode: "simple",
      shell_font: "Fira Code",
      code_font_family: null,
      terminal_font_family: null,
    });

    expect(resolved.codePreference).toBe("Fira Code");
    expect(resolved.terminalPreference).toBe("Fira Code");
    expect(resolved.codeFamily).toContain('"Fira Code"');
    expect(resolved.terminalFamily).toBe(resolved.codeFamily);
  });

  it("normalizes and safely quotes a single selected family", () => {
    expect(normalizeFontFamily("  Fira Code\u0000  ")).toBe("Fira Code");
    expect(normalizeFontFamily("   ")).toBeNull();
    expect(quoteFontFamily('A "Font"')).toBe('"A Font"');
    expect(quoteFontFamily("MONOSPACE")).toBe("monospace");
    expect(fontStack("Fira Code", DEFAULT_CODE_FONT_STACK)).toContain('"Fira Code"');
  });

  it("applies the complete CSS contract to the document root", () => {
    const root = document.createElement("html");
    const resolved = resolveTypographySettings({});

    applyTypography(root, resolved);

    expect(root.dataset.typographyMode).toBe("simple");
    expect(root.style.fontSize).toBe("16px");
    expect(root.style.getPropertyValue("--font-interface")).toBe(DEFAULT_INTERFACE_FONT_STACK);
    expect(root.style.getPropertyValue("--font-code")).toBe(DEFAULT_CODE_FONT_STACK);
    expect(root.style.getPropertyValue("--font-size-conversation")).toBe("14px");
    expect(root.style.getPropertyValue("--font-size-code")).toBe("13px");
    expect(root.style.getPropertyValue("--font-size-terminal")).toBe("13px");
  });
});
