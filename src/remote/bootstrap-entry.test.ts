import { describe, it, expect } from "vitest";

// `main.tsx` loads the web-remote runtime through this entry. If a module in
// its graph reads another's binding at top level while the two import each
// other, evaluation throws and the web client never leaves the splash.
describe("bootstrap-entry", () => {
  it("evaluates the web-remote module graph", async () => {
    const entry = await import("./bootstrap-entry");
    expect(entry.bootstrapRemote).toBeTypeOf("function");
  });
});
