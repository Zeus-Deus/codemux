/**
 * Suppress phantom replies emitted by xterm.js's VT parser to terminal queries
 * the shell sent at startup (DA, DSR/CPR, focus reports, DECRPM).
 *
 * xterm.js synthesizes some replies and emits them through `terminal.onData`,
 * which Codemux normally pipes back into the PTY. If those replies reach the
 * shell after the prompt is visible, they can be echoed as garbage like
 * `?62;4;9;22c`. Suppress the response-only CSI final bytes inside xterm's
 * parser before they reach onData.
 */
import type { Terminal } from "@xterm/xterm";

export function suppressQueryResponses(terminal: Terminal): () => void {
  const parser = terminal.parser;
  const disposables = [
    parser.registerCsiHandler({ final: "R" }, () => true),
    parser.registerCsiHandler({ final: "I" }, () => true),
    parser.registerCsiHandler({ final: "O" }, () => true),
    parser.registerCsiHandler({ intermediates: "$", final: "y" }, () => true),
  ];
  return () => {
    for (const disposable of disposables) {
      try {
        disposable.dispose();
      } catch {
        // ignore
      }
    }
  };
}
