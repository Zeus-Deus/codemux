/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, cleanup, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ModelPicker } from "./ModelPicker";

vi.mock("@/assets/preset-icons/claude.svg", () => ({
  default: "/mock/claude.svg",
}));
vi.mock("@/assets/preset-icons/codex.svg", () => ({
  default: "/mock/codex.svg",
}));

// `ModelPicker` now reads its model list from
// `provider-capabilities-store` via `capability-defaults.ts`. Stub a
// minimal payload that mirrors the Rust `capabilities.rs` data the
// real store would return, so the test's label assertions keep
// matching reality.
const CLAUDE_MODELS_STUB: Array<{
  id: string;
  label: string;
  description: string | null;
}> = [
  {
    id: "claude-opus-4-7",
    label: "Claude Opus 4.7",
    description: "Opus 4.8 with 1M context · Best for everyday, complex tasks",
  },
  {
    id: "claude-sonnet-4-6",
    label: "Claude Sonnet 4.6",
    description: "Sonnet 5 · Efficient for routine tasks",
  },
  // Kept without a description so the "no subtitle" rendering path
  // stays covered.
  { id: "claude-haiku-4-5", label: "Claude Haiku 4.5", description: null },
];
const CODEX_MODELS_STUB = [
  { id: "gpt-5.4", label: "GPT-5.4 (Codex)" },
  { id: "gpt-5.4-mini", label: "GPT-5.4 Mini" },
  { id: "gpt-5.3-codex", label: "GPT-5.3 Codex" },
];
const capsState = {
  claude: {
    models: CLAUDE_MODELS_STUB.map((m) => ({
      id: m.id,
      label: m.label,
      description: m.description,
      effort_levels: [],
      default_effort: null,
      prompt_injected_effort_levels: [],
      context_window_options: [],
      supports_adaptive_thinking: false,
      supports_thinking_toggle: false,
      supports_fast_mode: false,
    })),
    effort_granularity: "per_session",
    effort_label_map: {},
    permission_modes: [],
    default_permission_mode: null,
    permission_granularity: "per_session",
  },
  codex: {
    models: CODEX_MODELS_STUB.map((m) => ({
      id: m.id,
      label: m.label,
      description: null,
      effort_levels: [],
      default_effort: null,
      prompt_injected_effort_levels: [],
      context_window_options: [],
      supports_adaptive_thinking: false,
      supports_thinking_toggle: false,
      supports_fast_mode: false,
    })),
    effort_granularity: "per_session",
    effort_label_map: {},
    permission_modes: [],
    default_permission_mode: null,
    permission_granularity: "per_session",
  },
  claudeError: null,
  codexError: null,
};

const { refreshForIntent } = vi.hoisted(() => ({
  refreshForIntent: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/stores/provider-capabilities-store", () => ({
  useProviderCapabilities: Object.assign(
    vi.fn((selector: (s: unknown) => unknown) => selector(capsState)),
    { getState: () => capsState },
  ),
  selectCapabilities: (
    state: typeof capsState,
    provider: "claude" | "codex",
  ) => state[provider],
  refreshProviderCapabilitiesForIntent: refreshForIntent,
  // Mirror the real selector: exact id match, with the persisted
  // `"default"` alias falling back to the roster's first row.
  selectModel: (
    caps: { models: Array<{ id: string }> } | null,
    modelId: string | null,
  ) => {
    if (!caps || !modelId) return null;
    return (
      caps.models.find((m) => m.id === modelId) ??
      (modelId === "default" ? caps.models[0] ?? null : null)
    );
  },
}));

afterEach(() => {
  cleanup();
  refreshForIntent.mockClear();
});

describe("ModelPicker — render", () => {
  it("Claude provider: trigger pill shows the Claude logo", () => {
    const { container } = render(
      <TooltipProvider>
        <ModelPicker provider="claude" value={null} onChange={vi.fn()} />
      </TooltipProvider>,
    );
    const trigger = container.querySelector("button") as HTMLElement;
    const logo = trigger.querySelector("img") as HTMLImageElement;
    expect(logo).not.toBeNull();
    expect(logo.getAttribute("data-provider")).toBe("claude");
    expect(logo.getAttribute("src")).toContain("claude");
  });

  it("Codex provider: trigger pill shows the Codex logo", () => {
    const { container } = render(
      <TooltipProvider>
        <ModelPicker provider="codex" value={null} onChange={vi.fn()} />
      </TooltipProvider>,
    );
    const trigger = container.querySelector("button") as HTMLElement;
    const logo = trigger.querySelector("img") as HTMLImageElement;
    expect(logo.getAttribute("data-provider")).toBe("codex");
    expect(logo.getAttribute("src")).toContain("codex");
  });

  it("Claude provider: trigger label is the default Claude model", () => {
    const { container } = render(
      <TooltipProvider>
        <ModelPicker provider="claude" value={null} onChange={vi.fn()} />
      </TooltipProvider>,
    );
    const trigger = container.querySelector("button") as HTMLElement;
    expect(trigger.textContent).toContain("Claude");
  });

  it("Codex provider: trigger label names a Codex / GPT model", () => {
    const { container } = render(
      <TooltipProvider>
        <ModelPicker provider="codex" value={null} onChange={vi.fn()} />
      </TooltipProvider>,
    );
    const trigger = container.querySelector("button") as HTMLElement;
    expect(trigger.textContent).toMatch(/GPT|Codex/i);
  });

  it("honors the caller-provided value when it matches a known id", () => {
    const { container } = render(
      <TooltipProvider>
        <ModelPicker
          provider="claude"
          value="claude-sonnet-4-6"
          onChange={vi.fn()}
        />
      </TooltipProvider>,
    );
    const trigger = container.querySelector("button") as HTMLElement;
    expect(trigger.textContent).toContain("Sonnet");
  });

  it("disabled prop disables the trigger", () => {
    const { container } = render(
      <TooltipProvider>
        <ModelPicker provider="claude" value={null} onChange={vi.fn()} disabled />
      </TooltipProvider>,
    );
    const trigger = container.querySelector("button") as HTMLElement;
    expect(trigger.hasAttribute("disabled")).toBe(true);
  });

  it("does NOT render a search input (CommandInput removed)", () => {
    const { container } = render(
      <TooltipProvider>
        <ModelPicker provider="claude" value={null} onChange={vi.fn()} />
      </TooltipProvider>,
    );
    expect(
      container.querySelector("input[placeholder*='Search']"),
    ).toBeNull();
  });
});

describe("ModelPicker — interaction", () => {
  it("does not discover capabilities until the picker opens", async () => {
    const user = userEvent.setup();
    const { container } = render(
      <TooltipProvider>
        <ModelPicker provider="claude" value={null} onChange={vi.fn()} />
      </TooltipProvider>,
    );
    expect(refreshForIntent).not.toHaveBeenCalled();

    await user.click(container.querySelector("button") as HTMLElement);

    await waitFor(() => {
      expect(refreshForIntent).toHaveBeenCalledWith("claude");
    });
  });

  it("opening the popover lists every model for the provider and each row has the provider logo", async () => {
    const user = userEvent.setup();
    const { container } = render(
      <TooltipProvider>
        <ModelPicker provider="claude" value={null} onChange={vi.fn()} />
      </TooltipProvider>,
    );
    const trigger = container.querySelector("button") as HTMLElement;
    await user.click(trigger);
    const options = await screen.findAllByRole("option");
    expect(options.length).toBeGreaterThan(0);
    // Every row should carry a provider logo image.
    for (const opt of options) {
      const img = opt.querySelector("img") as HTMLImageElement | null;
      expect(img).not.toBeNull();
      expect(img!.getAttribute("data-provider")).toBe("claude");
    }
  });

  it("renders the model description as a row subtitle when present", async () => {
    const user = userEvent.setup();
    const { container } = render(
      <TooltipProvider>
        <ModelPicker provider="claude" value={null} onChange={vi.fn()} />
      </TooltipProvider>,
    );
    await user.click(container.querySelector("button") as HTMLElement);
    const options = await screen.findAllByRole("option");
    const opus = options.find((o) =>
      o.textContent?.includes("Claude Opus 4.7"),
    );
    expect(opus).toBeDefined();
    const subtitle = opus!.querySelector("[title]") as HTMLElement | null;
    expect(subtitle).not.toBeNull();
    expect(subtitle!.textContent).toBe(
      "Opus 4.8 with 1M context · Best for everyday, complex tasks",
    );
    expect(subtitle!).toHaveAttribute(
      "title",
      "Opus 4.8 with 1M context · Best for everyday, complex tasks",
    );
  });

  it("omits the subtitle for a model without a description", async () => {
    const user = userEvent.setup();
    const { container } = render(
      <TooltipProvider>
        <ModelPicker provider="claude" value={null} onChange={vi.fn()} />
      </TooltipProvider>,
    );
    await user.click(container.querySelector("button") as HTMLElement);
    const options = await screen.findAllByRole("option");
    const haiku = options.find((o) =>
      o.textContent?.includes("Claude Haiku 4.5"),
    );
    expect(haiku).toBeDefined();
    // No description → no subtitle element carrying a `title`.
    expect(haiku!.querySelector("[title]")).toBeNull();
  });

  it("clicking a row calls onChange with that model id", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const { container } = render(
      <TooltipProvider>
        <ModelPicker provider="claude" value={null} onChange={onChange} />
      </TooltipProvider>,
    );
    const trigger = container.querySelector("button") as HTMLElement;
    await user.click(trigger);
    const rows = await screen.findAllByRole("option");
    await user.click(rows[0]);
    expect(onChange).toHaveBeenCalledTimes(1);
    // The caller gets the SLUG, not the label — so we just verify
    // it's a non-empty string rather than binding to a specific id
    // (which would rot if the hardcoded list changes).
    const arg = onChange.mock.calls[0][0];
    expect(typeof arg).toBe("string");
    expect(arg.length).toBeGreaterThan(0);
  });

  it("arrow keys navigate + Enter selects after popover open", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const { container } = render(
      <TooltipProvider>
        <ModelPicker provider="claude" value={null} onChange={onChange} />
      </TooltipProvider>,
    );
    const trigger = container.querySelector("button") as HTMLElement;
    await user.click(trigger);
    await screen.findAllByRole("option");
    await user.keyboard("{ArrowDown}{Enter}");
    expect(onChange).toHaveBeenCalledTimes(1);
    const arg = onChange.mock.calls[0][0];
    expect(typeof arg).toBe("string");
    expect((arg as string).length).toBeGreaterThan(0);
  });
});

describe("ModelPicker — openSignal", () => {
  const renderSignal = (openSignal: number, disabled: boolean) =>
    render(
      <TooltipProvider>
        <ModelPicker
          provider="claude"
          value={null}
          onChange={vi.fn()}
          openSignal={openSignal}
          disabled={disabled}
        />
      </TooltipProvider>,
    );

  it("consumes the signal once — re-enabling after a consumed signal does not reopen", async () => {
    // Signal arrives while the picker is disabled → stays closed.
    const { rerender } = renderSignal(1, true);
    expect(screen.queryAllByRole("option")).toHaveLength(0);

    // Session becomes ready (`disabled` → false) WITHOUT a new signal.
    // The old effect reopened on every `disabled` transition; consuming
    // the signal via a ref keeps the picker closed here.
    rerender(
      <TooltipProvider>
        <ModelPicker
          provider="claude"
          value={null}
          onChange={vi.fn()}
          openSignal={1}
          disabled={false}
        />
      </TooltipProvider>,
    );
    expect(screen.queryAllByRole("option")).toHaveLength(0);

    // A genuine new `/model` press (incremented signal) still opens it.
    rerender(
      <TooltipProvider>
        <ModelPicker
          provider="claude"
          value={null}
          onChange={vi.fn()}
          openSignal={2}
          disabled={false}
        />
      </TooltipProvider>,
    );
    expect(await screen.findAllByRole("option")).not.toHaveLength(0);
  });
});
