import { describe, expect, it } from "vitest";
import { parseMessageDelivery, withMessageDelivery } from "./message-delivery";

describe("message delivery commands", () => {
  it.each(["queue", "steer", "interrupt"])("strips only the leading /%s control", (delivery) => {
    expect(parseMessageDelivery(` /${delivery.toUpperCase()}\nkeep /queue literal`)).toEqual({ delivery, text: "keep /queue literal", explicit: true });
    expect(parseMessageDelivery(`/${delivery}`)).toEqual({ delivery, text: "", explicit: true });
  });
  it.each(["/steering text", "/queue/file", "explain /steer", "`/interrupt` example"])("preserves literal content %s", (text) => {
    expect(parseMessageDelivery(text)).toEqual({ delivery: "queue", text, explicit: false });
  });
  it("switches delivery without nesting commands or changing the body", () => {
    expect(withMessageDelivery("/queue keep /steer literal", "steer")).toBe("/steer keep /steer literal");
  });
});
