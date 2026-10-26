import { describe, expect, it } from "vitest";
import { apiVersion, RENDERER_PLUGIN_CAPABILITIES, rendererPluginCompatibility } from "./compatibility";

describe("renderer plugin API compatibility", () => {
  it("keeps legacy plugins compatible without requiring new metadata", () => {
    expect(rendererPluginCompatibility({})).toEqual({ available: true, capabilities: RENDERER_PLUGIN_CAPABILITIES });
  });

  it("rejects unsupported API versions and required capabilities", () => {
    expect(rendererPluginCompatibility({ apiVersion: apiVersion + 1 }).available).toBe(false);
    const unsupported = rendererPluginCompatibility({ capabilities: ["ui.react", "future.native-editor"] });
    expect(unsupported.available).toBe(false);
    expect(unsupported.error).toContain("future.native-editor");
  });

  it("discovers supported optional capabilities without rejecting older hosts", () => {
    expect(rendererPluginCompatibility({
      apiVersion,
      capabilities: ["ui.react"],
      optionalCapabilities: ["ui.peek", "future.native-editor", "ui.react"],
    })).toEqual({ available: true, capabilities: ["ui.react", "ui.peek"] });
  });

  it("validates requirements supplied by untyped JavaScript", () => {
    const malformed: unknown[] = ["ui.react", 42, { name: "ui.react" }];
    // Reflect models the actual untyped export without blessing it as a typed plugin.
    for (const capabilities of malformed) {
      const requirements = Object.assign({}, { capabilities });
      expect(Reflect.apply(rendererPluginCompatibility, undefined, [requirements]).available).toBe(false);
    }
  });
});
