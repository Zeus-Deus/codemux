import { afterEach, expect, it, vi } from "vitest";
import { Terminal } from "@xterm/xterm";
import { createWritePump, DEFAULT_PUMP_BUDGET_BYTES } from "./terminal-write-pump";

afterEach(() => vi.useRealTimers());

it("parses multi-batch saved output at its saved width before fitting a different viewport", async () => {
  vi.useFakeTimers();
  const terminal = new Terminal({ cols: 35, rows: 38, allowProposedApi: true });
  const pump = createWritePump(data => terminal.write(data));
  const prompt = "PS D:\\a\\_temp\\codemux-native-ui-fixture\\synthetic-project> ";
  const history = ("history\r\n").repeat(Math.ceil(DEFAULT_PUMP_BUDGET_BYTES / 9) + 100);
  expect(history.length).toBeGreaterThan(DEFAULT_PUMP_BUDGET_BYTES);
  pump.enqueueString(history + prompt);
  let parsed: { cols: number; cursorX: number; tail: string } | undefined;
  const ready = (async () => {
    await pump.flush();
    await new Promise<void>(resolve => terminal.write("", resolve));
    const buffer = terminal.buffer.active;
    parsed = {
      cols: terminal.cols,
      cursorX: buffer.cursorX,
      tail: buffer.getLine(buffer.baseY + buffer.cursorY)?.translateToString(true) ?? "",
    };
    terminal.resize(70, 38);
  })();
  await vi.runAllTimersAsync();
  await ready;
  expect(parsed).toEqual({
    cols: 35,
    cursorX: prompt.length % 35,
    tail: prompt.slice(35),
  });
  expect(terminal.cols).toBe(70);
  pump.cancel();
  terminal.dispose();
});

it("releases a pending flush when replay is cancelled", async () => {
  vi.useFakeTimers();
  const write = vi.fn();
  const pump = createWritePump(write, { budgetBytes: 1, sliceChars: 1 });
  pump.enqueueString("history");
  const flushed = pump.flush();
  pump.cancel();
  await flushed;
  await vi.runAllTimersAsync();
  expect(write).toHaveBeenCalledTimes(1);
});
