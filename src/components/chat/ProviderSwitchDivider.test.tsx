/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

import type { ProviderSwitchItem } from "@/lib/agent-chat/types";

import { ProviderSwitchDivider } from "./ProviderSwitchDivider";

vi.mock("@/assets/preset-icons/claude.svg", () => ({ default: "/mock/claude.svg" }));
vi.mock("@/assets/preset-icons/codex.svg", () => ({ default: "/mock/codex.svg" }));

afterEach(() => {
  cleanup();
});

const item: ProviderSwitchItem = {
  kind: "provider_switch",
  id: "provider-switch-1",
  seq: 3,
  fromProvider: "claude",
  fromModel: "claude-opus-4-8",
  toProvider: "codex",
  toModel: null,
  handoffKind: "summary",
  truncated: false,
};

describe("ProviderSwitchDivider", () => {
  it("names both providers and what carried over", () => {
    render(<ProviderSwitchDivider item={item} />);
    const divider = screen.getByTestId("provider-switch-divider");
    expect(divider).toHaveTextContent("Claude · Claude Opus 4.8");
    expect(divider).toHaveTextContent("Codex");
    expect(divider).toHaveTextContent("summary carried over");
    expect(divider).toHaveAccessibleName(
      "Switched from Claude · Claude Opus 4.8 to Codex, summary carried over",
    );
  });
});
