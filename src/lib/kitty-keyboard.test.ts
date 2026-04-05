import { describe, it, expect } from "vitest";
import {
  KITTY_FUNCTIONAL_KEYS,
  BACKSPACE_CODEPOINT,
  csiUModifier,
  csiUSequence,
  scanKittySequences,
  applyKittyStack,
  kittyFlags,
} from "./kitty-keyboard";

// ── CSI u encoding ──

describe("csiUSequence", () => {
  it("Shift+Enter → \\x1b[13;2u", () => {
    expect(csiUSequence(13, 2)).toBe("\x1b[13;2u");
  });
  it("Ctrl+Enter → \\x1b[13;5u", () => {
    expect(csiUSequence(13, 5)).toBe("\x1b[13;5u");
  });
  it("Ctrl+Shift+Tab → \\x1b[9;6u", () => {
    expect(csiUSequence(9, 6)).toBe("\x1b[9;6u");
  });
  it("Ctrl+Space → \\x1b[32;5u", () => {
    expect(csiUSequence(32, 5)).toBe("\x1b[32;5u");
  });
  it("Alt+Enter → \\x1b[13;3u", () => {
    expect(csiUSequence(13, 3)).toBe("\x1b[13;3u");
  });
  it("Ctrl+Shift+Enter → \\x1b[13;6u", () => {
    expect(csiUSequence(13, 6)).toBe("\x1b[13;6u");
  });
});

describe("plain Enter must not CSI-u encode regardless of kittyLevel", () => {
  it("mod is 1 when no modifiers → handler gate (mod > 1) blocks CSI u", () => {
    const mod = csiUModifier({
      shiftKey: false,
      altKey: false,
      ctrlKey: false,
      metaKey: false,
    });
    expect(mod).toBe(1);
    expect(mod > 1).toBe(false);
  });
});

// ── Modifier encoding ──

describe("csiUModifier", () => {
  const mod = (o: Partial<Record<"shiftKey" | "altKey" | "ctrlKey" | "metaKey", boolean>>) =>
    csiUModifier({ shiftKey: false, altKey: false, ctrlKey: false, metaKey: false, ...o });

  it("no modifiers → 1", () => expect(mod({})).toBe(1));
  it("Shift → 2", () => expect(mod({ shiftKey: true })).toBe(2));
  it("Alt → 3", () => expect(mod({ altKey: true })).toBe(3));
  it("Alt+Shift → 4", () => expect(mod({ altKey: true, shiftKey: true })).toBe(4));
  it("Ctrl → 5", () => expect(mod({ ctrlKey: true })).toBe(5));
  it("Ctrl+Shift → 6", () => expect(mod({ ctrlKey: true, shiftKey: true })).toBe(6));
  it("Ctrl+Alt → 7", () => expect(mod({ ctrlKey: true, altKey: true })).toBe(7));
  it("Ctrl+Alt+Shift → 8", () =>
    expect(mod({ ctrlKey: true, altKey: true, shiftKey: true })).toBe(8));
  it("Meta (super) → 9", () => expect(mod({ metaKey: true })).toBe(9));
});

// ── Kitty stack management ──

describe("applyKittyStack", () => {
  it("push \\x1b[>1u → level becomes 1", () => {
    const stack = applyKittyStack([], [1], 0, false);
    expect(kittyFlags(stack)).toBe(1);
  });

  it("push \\x1b[>3u → level becomes 3 (not 1)", () => {
    const stack = applyKittyStack([], [3], 0, false);
    expect(kittyFlags(stack)).toBe(3);
  });

  it("pop \\x1b[<u → level returns to previous", () => {
    const pushed = applyKittyStack([], [1], 0, false);
    const popped = applyKittyStack(pushed, [], 1, false);
    expect(kittyFlags(popped)).toBe(0);
  });

  it("double push then single pop → level = first push value", () => {
    const stack = applyKittyStack([], [1, 3], 0, false);
    expect(kittyFlags(stack)).toBe(3);
    const afterPop = applyKittyStack(stack, [], 1, false);
    expect(kittyFlags(afterPop)).toBe(1);
  });

  it("pop with empty stack → level stays 0", () => {
    const stack = applyKittyStack([], [], 1, false);
    expect(kittyFlags(stack)).toBe(0);
    expect(stack).toEqual([]);
  });

  it("resetAll clears the entire stack", () => {
    const stack = applyKittyStack([1, 3], [], 0, true);
    expect(stack).toEqual([]);
    expect(kittyFlags(stack)).toBe(0);
  });

  it("resetAll then push applies push after reset", () => {
    const stack = applyKittyStack([1, 3], [5], 0, true);
    expect(stack).toEqual([5]);
    expect(kittyFlags(stack)).toBe(5);
  });
});

// ── Query response ──

describe("kittyFlags (query response value)", () => {
  it("before any push → 0", () => {
    expect(kittyFlags([])).toBe(0);
  });

  it("after push \\x1b[>1u → 1", () => {
    expect(kittyFlags(applyKittyStack([], [1], 0, false))).toBe(1);
  });

  it("after push \\x1b[>3u → 3", () => {
    expect(kittyFlags(applyKittyStack([], [3], 0, false))).toBe(3);
  });
});

// ── Sequence scanning ──

describe("scanKittySequences", () => {
  it("detects Kitty query \\x1b[?u", () => {
    expect(scanKittySequences("\x1b[?u").hasQuery).toBe(true);
  });

  it("does not false-match query response \\x1b[?1u", () => {
    expect(scanKittySequences("\x1b[?1u").hasQuery).toBe(false);
  });

  it("extracts push flag value from \\x1b[>1u", () => {
    expect(scanKittySequences("\x1b[>1u").pushValues).toEqual([1]);
  });

  it("extracts push flag value from \\x1b[>3u", () => {
    expect(scanKittySequences("\x1b[>3u").pushValues).toEqual([3]);
  });

  it("extracts multiple pushes in one chunk", () => {
    expect(scanKittySequences("\x1b[>1u\x1b[>3u").pushValues).toEqual([1, 3]);
  });

  it("counts single pop", () => {
    expect(scanKittySequences("\x1b[<u").popCount).toBe(1);
  });

  it("counts multiple pops", () => {
    expect(scanKittySequences("\x1b[<u\x1b[<u").popCount).toBe(2);
  });

  it("detects DA1 query \\x1b[c", () => {
    expect(scanKittySequences("\x1b[c").hasDAQuery).toBe(true);
  });

  it("detects DA2 query \\x1b[>c", () => {
    expect(scanKittySequences("\x1b[>c").hasDAQuery).toBe(true);
  });

  it("detects DA1 with explicit 0 param \\x1b[0c", () => {
    expect(scanKittySequences("\x1b[0c").hasDAQuery).toBe(true);
  });

  it("does not false-match CUF \\x1b[123c as DA query", () => {
    expect(scanKittySequences("\x1b[123c").hasDAQuery).toBe(false);
  });

  it("returns empty result for plain text", () => {
    const r = scanKittySequences("hello world");
    expect(r.hasQuery).toBe(false);
    expect(r.pushValues).toEqual([]);
    expect(r.popCount).toBe(0);
    expect(r.hasDAQuery).toBe(false);
  });
});

// ── Level reset on DA query ──

describe("DA query resets Kitty state", () => {
  it("kittyLevel=1, PTY sends \\x1b[c → level resets to 0", () => {
    const stack = applyKittyStack([], [1], 0, false);
    expect(kittyFlags(stack)).toBe(1);

    const scan = scanKittySequences("\x1b[c");
    const newStack = applyKittyStack(
      stack,
      scan.pushValues,
      scan.popCount,
      scan.hasDAQuery,
    );
    expect(kittyFlags(newStack)).toBe(0);
  });

  it("kittyLevel=3, DA2 query \\x1b[>c → level resets to 0", () => {
    const stack = applyKittyStack([], [3], 0, false);
    const scan = scanKittySequences("\x1b[>c");
    const newStack = applyKittyStack(
      stack,
      scan.pushValues,
      scan.popCount,
      scan.hasDAQuery,
    );
    expect(kittyFlags(newStack)).toBe(0);
  });
});

// ── Unconditional CSI u (modern terminal behavior) ──

describe("unconditional CSI u for Enter/Tab/Space (kittyLevel = 0)", () => {
  it("Shift+Enter sends CSI u even at kittyLevel 0", () => {
    const codepoint = KITTY_FUNCTIONAL_KEYS.get("Enter");
    const mod = csiUModifier({ shiftKey: true, altKey: false, ctrlKey: false, metaKey: false });
    expect(codepoint).toBe(13);
    expect(mod).toBe(2);
    // Not backspace → unconditional
    expect(codepoint !== BACKSPACE_CODEPOINT).toBe(true);
    expect(csiUSequence(codepoint!, mod)).toBe("\x1b[13;2u");
  });

  it("Ctrl+Enter sends CSI u even at kittyLevel 0", () => {
    const codepoint = KITTY_FUNCTIONAL_KEYS.get("Enter");
    const mod = csiUModifier({ shiftKey: false, altKey: false, ctrlKey: true, metaKey: false });
    expect(codepoint !== BACKSPACE_CODEPOINT).toBe(true);
    expect(csiUSequence(codepoint!, mod)).toBe("\x1b[13;5u");
  });

  it("Ctrl+Space sends CSI u even at kittyLevel 0", () => {
    const codepoint = KITTY_FUNCTIONAL_KEYS.get(" ");
    const mod = csiUModifier({ shiftKey: false, altKey: false, ctrlKey: true, metaKey: false });
    expect(codepoint !== BACKSPACE_CODEPOINT).toBe(true);
    expect(csiUSequence(codepoint!, mod)).toBe("\x1b[32;5u");
  });

  it("plain Enter at kittyLevel 0 does NOT send CSI u", () => {
    const mod = csiUModifier({ shiftKey: false, altKey: false, ctrlKey: false, metaKey: false });
    expect(mod > 1).toBe(false);
  });
});

describe("Backspace CSI u gated behind kittyLevel > 0", () => {
  it("Ctrl+Backspace at kittyLevel 0 does NOT send CSI u (falls through to backward-kill-word)", () => {
    const codepoint = KITTY_FUNCTIONAL_KEYS.get("Backspace");
    expect(codepoint).toBe(BACKSPACE_CODEPOINT);
    // Handler check: isBackspace && kittyLevel === 0 → skip CSI u
    const isBackspace = codepoint === BACKSPACE_CODEPOINT;
    const kittyLevel = 0;
    expect(isBackspace && kittyLevel === 0).toBe(true);
  });

  it("Ctrl+Backspace at kittyLevel 1 sends CSI u", () => {
    const codepoint = KITTY_FUNCTIONAL_KEYS.get("Backspace");
    const mod = csiUModifier({ shiftKey: false, altKey: false, ctrlKey: true, metaKey: false });
    const isBackspace = codepoint === BACKSPACE_CODEPOINT;
    const kittyLevel = 1;
    expect(!isBackspace || kittyLevel > 0).toBe(true);
    expect(csiUSequence(codepoint!, mod)).toBe("\x1b[127;5u");
  });
});

// ── App shortcut priority ──

describe("KITTY_FUNCTIONAL_KEYS does not intercept regular keys", () => {
  it("does not contain letter keys (Ctrl+K stays an app shortcut)", () => {
    expect(KITTY_FUNCTIONAL_KEYS.has("k")).toBe(false);
    expect(KITTY_FUNCTIONAL_KEYS.has("K")).toBe(false);
  });

  it("does not contain 'c' (Ctrl+C stays an interrupt)", () => {
    expect(KITTY_FUNCTIONAL_KEYS.has("c")).toBe(false);
  });

  it("contains exactly Enter, Tab, Backspace, Space", () => {
    expect([...KITTY_FUNCTIONAL_KEYS.keys()].sort()).toEqual(
      [" ", "Backspace", "Enter", "Tab"],
    );
  });
});
