import { describe, expect, it } from "vitest";

import { leafModelName } from "./footer-trigger";

describe("leafModelName", () => {
  it("keeps the last segment of a routed id", () => {
    expect(leafModelName("openrouter/qwen3-coder")).toBe("qwen3-coder");
  });

  it("drops a leading brand the provider logo already shows", () => {
    expect(leafModelName("Claude Opus 4.8", "Claude")).toBe("Opus 4.8");
    expect(leafModelName("claude opus 4.8", "Claude")).toBe("opus 4.8");
  });

  it("keeps the brand when only a version would remain", () => {
    expect(leafModelName("Grok 4", "Grok")).toBe("Grok 4");
  });

  it("leaves labels from other brands alone", () => {
    expect(leafModelName("GPT-5.6 Luna", "Codex")).toBe("GPT-5.6 Luna");
  });
});
