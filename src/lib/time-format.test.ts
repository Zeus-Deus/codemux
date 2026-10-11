import { describe, expect, it } from "vitest";
import { withTimeFormat } from "./time-format";

describe("withTimeFormat", () => {
  const base: Intl.DateTimeFormatOptions = { hour: "2-digit", minute: "2-digit" };

  it("leaves the caller's options alone for the system format", () => {
    const pinned = { ...base, hourCycle: "h23" as const };
    expect(withTimeFormat(pinned, "system")).toBe(pinned);
  });

  it("forces a 12-hour clock over a pinned hour cycle", () => {
    expect(withTimeFormat({ ...base, hourCycle: "h23" }, "12h")).toEqual({
      ...base,
      hour12: true,
    });
  });

  it("forces a 24-hour clock over hour12", () => {
    const options = withTimeFormat({ ...base, hour12: true }, "24h");
    expect(options).toEqual({ ...base, hourCycle: "h23" });
    const at = new Date(2026, 0, 1, 15, 7);
    expect(at.toLocaleTimeString("en-US", options)).toBe("15:07");
  });
});
