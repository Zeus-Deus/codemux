import { describe, expect, it } from "vitest";

import {
  detectLaunchFamily,
  familyToProviderKind,
  GEMINI_MODELS,
  MODEL_SEARCH_THRESHOLD,
  parseBakedModel,
  REASONING_FLAG_FAMILIES,
} from "./launch-models";

describe("detectLaunchFamily", () => {
  it("detects each known agent family from its preset command", () => {
    expect(detectLaunchFamily("claude --dangerously-skip-permissions")).toBe(
      "claude",
    );
    expect(detectLaunchFamily("codex --full-auto")).toBe("codex");
    expect(detectLaunchFamily("opencode")).toBe("opencode");
    expect(detectLaunchFamily("gemini --yolo")).toBe("gemini");
  });

  it("returns null for unmodeled binaries and empty input", () => {
    expect(detectLaunchFamily("pi")).toBeNull();
    expect(detectLaunchFamily("npx some-agent")).toBeNull();
    expect(detectLaunchFamily("")).toBeNull();
    expect(detectLaunchFamily(null)).toBeNull();
    expect(detectLaunchFamily(undefined)).toBeNull();
  });

  it("strips a directory path and leading env assignments", () => {
    expect(detectLaunchFamily("/usr/local/bin/claude --foo")).toBe("claude");
    expect(detectLaunchFamily("FOO=bar BAZ=qux codex --full-auto")).toBe(
      "codex",
    );
  });
});

describe("familyToProviderKind", () => {
  it("maps the chat-driver families to their provider kind", () => {
    expect(familyToProviderKind("claude")).toBe("claude");
    expect(familyToProviderKind("codex")).toBe("codex");
    expect(familyToProviderKind("opencode")).toBe("opencode");
  });

  it("returns null for Gemini (no chat driver)", () => {
    expect(familyToProviderKind("gemini")).toBeNull();
  });
});

describe("parseBakedModel", () => {
  it("extracts a model baked into the preset command", () => {
    expect(parseBakedModel("claude --model opus")).toBe("opus");
    expect(parseBakedModel("codex -m gpt-5.4 --full-auto")).toBe("gpt-5.4");
    expect(parseBakedModel("claude --model=sonnet")).toBe("sonnet");
  });

  it("returns null when no model flag is present", () => {
    expect(parseBakedModel("claude --dangerously-skip-permissions")).toBeNull();
    expect(parseBakedModel("opencode")).toBeNull();
    expect(parseBakedModel(null)).toBeNull();
  });
});

describe("REASONING_FLAG_FAMILIES", () => {
  it("covers exactly the CLIs that expose a reasoning flag", () => {
    expect(REASONING_FLAG_FAMILIES.has("claude")).toBe(true);
    expect(REASONING_FLAG_FAMILIES.has("codex")).toBe(true);
    expect(REASONING_FLAG_FAMILIES.has("opencode")).toBe(false);
    expect(REASONING_FLAG_FAMILIES.has("gemini")).toBe(false);
  });
});

describe("GEMINI_MODELS", () => {
  it("ships a non-empty maintained model list", () => {
    expect(GEMINI_MODELS.length).toBeGreaterThan(0);
    for (const model of GEMINI_MODELS) {
      expect(model.id).toBeTruthy();
      expect(model.label).toBeTruthy();
    }
  });
});

describe("MODEL_SEARCH_THRESHOLD", () => {
  it("is large enough that Claude/Codex lists stay flat", () => {
    // Claude and Codex ship well under 10 models; OpenCode's federated
    // list blows past it. The threshold gates the search input.
    expect(MODEL_SEARCH_THRESHOLD).toBeGreaterThanOrEqual(8);
  });
});
