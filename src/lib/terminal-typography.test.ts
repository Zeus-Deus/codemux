import { describe, expect, it, vi } from "vitest";

import { applyTerminalTypography } from "./terminal-typography";

describe("live terminal typography", () => {
  it("updates the existing renderer and invalidates its glyph atlas", () => {
    const terminal = {
      options: { fontFamily: "monospace", fontSize: 13 },
      clearTextureAtlas: vi.fn(),
    };

    expect(applyTerminalTypography(terminal, '"Fira Code", monospace', 16)).toBe(true);
    expect(terminal.options).toEqual({ fontFamily: '"Fira Code", monospace', fontSize: 16 });
    expect(terminal.clearTextureAtlas).toHaveBeenCalledTimes(1);
  });

  it("does nothing for repeated settings events", () => {
    const terminal = {
      options: { fontFamily: "monospace", fontSize: 13 },
      clearTextureAtlas: vi.fn(),
    };

    expect(applyTerminalTypography(terminal, "monospace", 13)).toBe(false);
    expect(terminal.clearTextureAtlas).not.toHaveBeenCalled();
  });
});
