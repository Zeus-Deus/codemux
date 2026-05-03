/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type {
  ChatModelInfo,
  ProviderChatCapabilities,
} from "@/tauri/types";
import { useProviderCapabilities } from "@/stores/provider-capabilities-store";
import { usePickerFavorites } from "@/stores/picker-favorites-store";

import { MultiProviderModelPicker } from "./MultiProviderModelPicker";

// ── Fixtures ───────────────────────────────────────────────────────

function makeModel(overrides: Partial<ChatModelInfo>): ChatModelInfo {
  return {
    id: "model-x",
    label: "Model X",
    description: null,
    effort_levels: [],
    default_effort: null,
    prompt_injected_effort_levels: [],
    context_window_options: [],
    supports_adaptive_thinking: false,
    supports_thinking_toggle: false,
    supports_fast_mode: false,
    supports_images: false,
    sub_provider: null,
    is_free: false,
    ...overrides,
  };
}

function makeCaps(models: ChatModelInfo[]): ProviderChatCapabilities {
  return {
    models,
    effort_granularity: "per_session",
    effort_label_map: {},
    permission_modes: [],
    default_permission_mode: null,
    permission_granularity: "per_session",
  };
}

const CLAUDE_CAPS = makeCaps([
  makeModel({ id: "claude-opus-4-7", label: "Claude Opus 4.7" }),
  makeModel({ id: "claude-haiku-4-5", label: "Claude Haiku 4.5" }),
]);

const CODEX_CAPS = makeCaps([
  makeModel({ id: "gpt-5.4", label: "GPT-5.4 (Codex)" }),
  makeModel({ id: "gpt-5.4-mini", label: "GPT-5.4 Mini" }),
  makeModel({ id: "gpt-5.3-codex", label: "GPT-5.3 Codex" }),
  makeModel({ id: "codex-mini-latest", label: "codex-mini-latest" }),
]);

const OPENCODE_CAPS = makeCaps([
  makeModel({
    id: "openai/gpt-5",
    label: "GPT-5",
    sub_provider: "openai",
  }),
  makeModel({
    id: "anthropic/claude-sonnet-4-6",
    label: "Claude Sonnet 4.6",
    sub_provider: "anthropic",
  }),
  makeModel({
    id: "openrouter/x-ai/grok-2",
    label: "Grok 2",
    sub_provider: "openrouter",
  }),
]);

function seedStore(opts: {
  claude?: ProviderChatCapabilities | null;
  codex?: ProviderChatCapabilities | null;
  opencode?: ProviderChatCapabilities | null;
  claudeError?: string | null;
  codexError?: string | null;
  opencodeError?: string | null;
} = {}) {
  useProviderCapabilities.setState({
    claude: opts.claude ?? null,
    codex: opts.codex ?? null,
    opencode: opts.opencode ?? null,
    claudeError: opts.claudeError ?? null,
    codexError: opts.codexError ?? null,
    opencodeError: opts.opencodeError ?? null,
    loaded: false,
  });
}

beforeEach(() => {
  seedStore({
    claude: CLAUDE_CAPS,
    codex: CODEX_CAPS,
    opencode: OPENCODE_CAPS,
  });
  // Clear any favorites from a previous test so the search-boost
  // assertions start from a known sort order.
  localStorage.clear();
  usePickerFavorites.setState({ favorites: [] });
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  usePickerFavorites.setState({ favorites: [] });
});

// ── Helpers ────────────────────────────────────────────────────────

type Props = Parameters<typeof MultiProviderModelPicker>[0];

function renderPicker(overrides: Partial<Props> = {}) {
  const onProviderModelChange = vi.fn();
  const utils = render(
    <MultiProviderModelPicker
      provider="claude"
      model="claude-opus-4-7"
      onProviderModelChange={onProviderModelChange}
      {...overrides}
    />,
  );
  return { ...utils, onProviderModelChange };
}

async function openPicker(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByTestId("multi-provider-model-picker-trigger"));
  // Wait for the popover content to mount.
  await screen.findByPlaceholderText("Search models...");
}

// ── Tests ──────────────────────────────────────────────────────────

describe("MultiProviderModelPicker — trigger", () => {
  it("shows the active model label when capabilities are loaded", () => {
    renderPicker({ provider: "claude", model: "claude-opus-4-7" });
    expect(screen.getByTestId("multi-provider-model-picker-trigger"))
      .toHaveTextContent("Claude Opus 4.7");
  });

  it("renders sub_provider · label when active model is OpenCode", () => {
    renderPicker({ provider: "opencode", model: "openai/gpt-5" });
    const trigger = screen.getByTestId("multi-provider-model-picker-trigger");
    expect(trigger).toHaveTextContent("openai");
    expect(trigger).toHaveTextContent("GPT-5");
  });

  it("falls back to 'Loading…' when caps + model are absent", () => {
    seedStore({ claude: null, codex: null, opencode: null });
    renderPicker({ provider: "claude", model: null });
    expect(screen.getByTestId("multi-provider-model-picker-trigger"))
      .toHaveTextContent("Loading…");
  });

  it("falls back to the raw model id if the model isn't in the caps list", () => {
    renderPicker({ provider: "claude", model: "claude-future-9000" });
    expect(screen.getByTestId("multi-provider-model-picker-trigger"))
      .toHaveTextContent("claude-future-9000");
  });

  it("does not open when disabled", async () => {
    const user = userEvent.setup();
    renderPicker({ disabled: true });
    await user.click(screen.getByTestId("multi-provider-model-picker-trigger"));
    expect(
      screen.queryByPlaceholderText("Search models..."),
    ).not.toBeInTheDocument();
  });
});

describe("MultiProviderModelPicker — provider rail", () => {
  it("renders all three providers in the rail", async () => {
    const user = userEvent.setup();
    renderPicker();
    await openPicker(user);
    expect(screen.getByTestId("provider-rail-claude")).toBeInTheDocument();
    expect(screen.getByTestId("provider-rail-codex")).toBeInTheDocument();
    expect(screen.getByTestId("provider-rail-opencode")).toBeInTheDocument();
  });

  it("marks the active provider as selected", async () => {
    const user = userEvent.setup();
    renderPicker({ provider: "codex", model: "gpt-5.4" });
    await openPicker(user);
    expect(screen.getByTestId("provider-rail-codex")).toHaveAttribute(
      "data-selected",
      "true",
    );
    expect(
      screen.getByTestId("provider-rail-claude").getAttribute("data-selected"),
    ).toBeNull();
  });

  it("clicking a rail entry filters the model list", async () => {
    const user = userEvent.setup();
    renderPicker();
    await openPicker(user);

    // Sanity: Claude is selected, Claude models present, Codex models absent.
    // Use `getAllByText` because the trigger label also reads "Claude
    // Opus 4.7"; we scope to model rows by `data-testid`.
    const initialRows = screen.getAllByTestId("multi-provider-model-row");
    expect(
      initialRows.some((r) => r.textContent?.includes("Claude Opus 4.7")),
    ).toBe(true);
    expect(
      initialRows.some((r) => r.textContent?.includes("GPT-5.4")),
    ).toBe(false);

    await user.click(screen.getByTestId("provider-rail-codex"));

    // After switch: Codex models present, Claude gone.
    await waitFor(() => {
      const rows = screen.getAllByTestId("multi-provider-model-row");
      expect(rows.some((r) => r.textContent?.includes("GPT-5.4 (Codex)"))).toBe(
        true,
      );
      expect(
        rows.some((r) => r.textContent?.includes("Claude Opus 4.7")),
      ).toBe(false);
    });
  });

  it("rail icon is dimmed and tooltip flags 'Not installed' when OpenCode is missing", async () => {
    seedStore({
      claude: CLAUDE_CAPS,
      codex: CODEX_CAPS,
      opencode: null,
      opencodeError: "opencode_not_installed",
    });
    const user = userEvent.setup();
    renderPicker();
    await openPicker(user);

    const opencodeBtn = screen.getByTestId("provider-rail-opencode");
    // Visual cue: `data-unavailable="true"` is the test-stable hook
    // for the dimmed-icon styling. The Tailwind opacity-40 class is
    // applied via the same gate.
    expect(opencodeBtn).toHaveAttribute("data-unavailable", "true");

    // Sanity: Claude & Codex rail icons stay un-dimmed.
    expect(
      screen
        .getByTestId("provider-rail-claude")
        .getAttribute("data-unavailable"),
    ).toBeNull();
    expect(
      screen
        .getByTestId("provider-rail-codex")
        .getAttribute("data-unavailable"),
    ).toBeNull();

    // The rail entry is still clickable so the user can see the
    // empty-state install hint.
    await user.click(opencodeBtn);
    expect(
      await screen.findByText("OpenCode not detected on your system"),
    ).toBeInTheDocument();
  });

  it("rail icon is dimmed when Codex returns a prefixed not_authenticated error", async () => {
    seedStore({
      claude: CLAUDE_CAPS,
      codex: null,
      opencode: OPENCODE_CAPS,
      codexError: "codex_not_authenticated: Run `codex login` and try again.",
    });
    const user = userEvent.setup();
    renderPicker();
    await openPicker(user);

    const codexBtn = screen.getByTestId("provider-rail-codex");
    // The harvest returns a prefixed string with a colon + hint; the
    // rail must still recognise the prefix and dim the icon (the old
    // implementation only special-cased the bare `opencode_not_installed`
    // token and let Codex fall through to a no-op).
    expect(codexBtn).toHaveAttribute("data-unavailable", "true");
  });

  it("OpenCode rail shows federated rows with sub_provider subtitles", async () => {
    const user = userEvent.setup();
    renderPicker();
    await openPicker(user);
    await user.click(screen.getByTestId("provider-rail-opencode"));

    // Each OpenCode row's secondary line should read "OpenCode · {sub_provider}".
    expect(await screen.findByText("OpenCode · openai")).toBeInTheDocument();
    expect(screen.getByText("OpenCode · anthropic")).toBeInTheDocument();
    expect(screen.getByText("OpenCode · openrouter")).toBeInTheDocument();
  });
});

describe("MultiProviderModelPicker — search", () => {
  it("filtering by query collapses provider grouping into a flat list", async () => {
    const user = userEvent.setup();
    renderPicker({ provider: "claude" });
    await openPicker(user);

    await user.type(
      screen.getByPlaceholderText("Search models..."),
      "gpt",
    );

    // Codex AND OpenCode/openai-namespaced models should now appear,
    // even though we're on the Claude rail. Scope to row testid so
    // the trigger's label can't shadow the assertion.
    await waitFor(() => {
      const rows = screen.getAllByTestId("multi-provider-model-row");
      expect(rows.some((r) => r.textContent?.includes("GPT-5.4 (Codex)"))).toBe(
        true,
      );
      // GPT-5 (the OpenCode federated model) should also surface.
      expect(rows.some((r) => r.textContent?.match(/\bGPT-5\b/))).toBe(true);
      // Claude rows should be filtered out (they don't match "gpt").
      expect(
        rows.some((r) => r.textContent?.includes("Claude Opus 4.7")),
      ).toBe(false);
    });
  });

  it("matches on label, id, and sub_provider", async () => {
    const user = userEvent.setup();
    renderPicker();
    await openPicker(user);

    // Match by sub_provider name only — "openrouter" lives in
    // sub_provider, not in the label or id.
    await user.type(
      screen.getByPlaceholderText("Search models..."),
      "openrouter",
    );
    await waitFor(() => {
      const rows = screen.getAllByTestId("multi-provider-model-row");
      expect(
        rows.some((r) => r.textContent?.includes("OpenCode · openrouter")),
      ).toBe(true);
      expect(
        rows.some((r) => r.textContent?.includes("Claude Opus 4.7")),
      ).toBe(false);
    });
  });

  it("renders a no-match empty state when the query matches nothing", async () => {
    const user = userEvent.setup();
    renderPicker();
    await openPicker(user);

    await user.type(
      screen.getByPlaceholderText("Search models..."),
      "nonexistent-zzz",
    );

    expect(
      await screen.findByText(/No models match/i),
    ).toBeInTheDocument();
    expect(screen.getByText('"nonexistent-zzz"')).toBeInTheDocument();
  });
});

describe("MultiProviderModelPicker — empty + error states", () => {
  it("shows 'OpenCode not detected' when opencode_not_installed", async () => {
    seedStore({
      claude: CLAUDE_CAPS,
      codex: CODEX_CAPS,
      opencode: null,
      opencodeError: "opencode_not_installed",
    });
    const user = userEvent.setup();
    renderPicker();
    await openPicker(user);
    await user.click(screen.getByTestId("provider-rail-opencode"));

    expect(
      await screen.findByText("OpenCode not detected on your system"),
    ).toBeInTheDocument();
  });

  it("shows generic harvest failure for other OpenCode errors", async () => {
    seedStore({
      claude: CLAUDE_CAPS,
      codex: CODEX_CAPS,
      opencode: null,
      opencodeError: "ready_timeout_after_10000ms",
    });
    const user = userEvent.setup();
    renderPicker();
    await openPicker(user);
    await user.click(screen.getByTestId("provider-rail-opencode"));

    expect(
      await screen.findByText(/OpenCode harvest failed/i),
    ).toBeInTheDocument();
    expect(
      screen.getByText("ready_timeout_after_10000ms"),
    ).toBeInTheDocument();
  });

  it("shows 'No connected providers' when OpenCode loaded with zero models", async () => {
    seedStore({
      claude: CLAUDE_CAPS,
      codex: CODEX_CAPS,
      opencode: makeCaps([]),
    });
    const user = userEvent.setup();
    renderPicker();
    await openPicker(user);
    await user.click(screen.getByTestId("provider-rail-opencode"));

    expect(
      await screen.findByText("No connected providers"),
    ).toBeInTheDocument();
    expect(screen.getByText("opencode auth login")).toBeInTheDocument();
  });

  it("shows skeletons when the rail provider's caps are still null", async () => {
    seedStore({
      claude: null,
      codex: CODEX_CAPS,
      opencode: OPENCODE_CAPS,
    });
    const user = userEvent.setup();
    renderPicker({ provider: "claude", model: null });
    await openPicker(user);

    expect(
      screen.getByTestId("multi-provider-loading-claude"),
    ).toBeInTheDocument();
  });

  it("shows 'Codex not detected' when codex_not_installed", async () => {
    seedStore({
      claude: CLAUDE_CAPS,
      codex: null,
      opencode: OPENCODE_CAPS,
      codexError:
        "codex_not_installed: Install Codex CLI from https://github.com/openai/codex and ensure `codex` is on PATH.",
    });
    const user = userEvent.setup();
    renderPicker();
    await openPicker(user);
    await user.click(screen.getByTestId("provider-rail-codex"));

    expect(
      await screen.findByText("Codex not detected on your system"),
    ).toBeInTheDocument();
  });

  it("shows 'Codex is not signed in' when codex_not_authenticated", async () => {
    seedStore({
      claude: CLAUDE_CAPS,
      codex: null,
      opencode: OPENCODE_CAPS,
      codexError:
        "codex_not_authenticated: Run `codex login` and try again.",
    });
    const user = userEvent.setup();
    renderPicker();
    await openPicker(user);
    await user.click(screen.getByTestId("provider-rail-codex"));

    expect(
      await screen.findByText("Codex is not signed in"),
    ).toBeInTheDocument();
    // Hint surfaces the actual command the user has to run.
    expect(screen.getByText("codex login")).toBeInTheDocument();
  });

  it("shows generic 'Codex harvest failed' for codex_harvest_failed", async () => {
    seedStore({
      claude: CLAUDE_CAPS,
      codex: null,
      opencode: OPENCODE_CAPS,
      codexError: "codex_harvest_failed: spawn failed: ENOENT",
    });
    const user = userEvent.setup();
    renderPicker();
    await openPicker(user);
    await user.click(screen.getByTestId("provider-rail-codex"));

    expect(
      await screen.findByText(/Codex harvest failed/i),
    ).toBeInTheDocument();
    expect(
      screen.getByText("spawn failed: ENOENT"),
    ).toBeInTheDocument();
  });
});

describe("MultiProviderModelPicker — selection", () => {
  it("selecting a model in the same provider fires the change handler", async () => {
    const user = userEvent.setup();
    const { onProviderModelChange } = renderPicker({
      provider: "claude",
      model: "claude-opus-4-7",
    });
    await openPicker(user);

    await user.click(screen.getByText("Claude Haiku 4.5"));

    expect(onProviderModelChange).toHaveBeenCalledWith(
      "claude",
      "claude-haiku-4-5",
    );
  });

  it("selecting a model in a different provider fires both kind+model", async () => {
    const user = userEvent.setup();
    const { onProviderModelChange } = renderPicker({
      provider: "claude",
      model: "claude-opus-4-7",
    });
    await openPicker(user);
    await user.click(screen.getByTestId("provider-rail-codex"));

    await user.click(await screen.findByText("GPT-5.4 (Codex)"));

    expect(onProviderModelChange).toHaveBeenCalledWith("codex", "gpt-5.4");
  });

  it("selecting a model closes the popover", async () => {
    const user = userEvent.setup();
    renderPicker();
    await openPicker(user);

    await user.click(screen.getByText("Claude Haiku 4.5"));

    await waitFor(() => {
      expect(
        screen.queryByPlaceholderText("Search models..."),
      ).not.toBeInTheDocument();
    });
  });

  it("selecting an OpenCode federated model passes the namespaced slug", async () => {
    const user = userEvent.setup();
    const { onProviderModelChange } = renderPicker();
    await openPicker(user);
    await user.click(screen.getByTestId("provider-rail-opencode"));

    await user.click(await screen.findByText("Grok 2"));

    expect(onProviderModelChange).toHaveBeenCalledWith(
      "opencode",
      "openrouter/x-ai/grok-2",
    );
  });
});

// ── Stage 6 — favorites ────────────────────────────────────────────

describe("MultiProviderModelPicker — favorites", () => {
  it("renders a star toggle on every model row", async () => {
    const user = userEvent.setup();
    renderPicker();
    await openPicker(user);

    const stars = screen.getAllByTestId("model-row-favorite-toggle");
    // CLAUDE_CAPS has 2 models; only Claude is on the rail by
    // default. Score-boost search expansion is exercised separately.
    expect(stars.length).toBe(CLAUDE_CAPS.models.length);
  });

  it("favorited rows render with the star filled (data-favorite)", async () => {
    const user = userEvent.setup();
    // Pre-seed one favorite so the picker opens already populated.
    usePickerFavorites
      .getState()
      .toggle("claude", "claude-opus-4-7");
    renderPicker();
    await openPicker(user);

    const stars = screen.getAllByTestId("model-row-favorite-toggle");
    const filled = stars.filter(
      (s) => s.getAttribute("data-favorite") === "true",
    );
    const empty = stars.filter(
      (s) => s.getAttribute("data-favorite") !== "true",
    );
    expect(filled).toHaveLength(1);
    expect(empty.length).toBeGreaterThanOrEqual(1);
  });

  it("clicking a star toggles favorite state without selecting the row", async () => {
    const user = userEvent.setup();
    const { onProviderModelChange } = renderPicker({
      provider: "claude",
      model: "claude-opus-4-7",
    });
    await openPicker(user);

    // Click the star on the *non-active* row (Haiku) so we can
    // confirm the click didn't trigger a selection.
    const rows = screen.getAllByTestId("multi-provider-model-row");
    const haikuRow = rows.find((r) =>
      r.textContent?.includes("Claude Haiku 4.5"),
    );
    expect(haikuRow).toBeDefined();
    const haikuStar = haikuRow!.querySelector(
      '[data-testid="model-row-favorite-toggle"]',
    ) as HTMLElement | null;
    expect(haikuStar).not.toBeNull();

    await user.click(haikuStar!);

    expect(
      usePickerFavorites.getState().isFavorite("claude", "claude-haiku-4-5"),
    ).toBe(true);
    // The selection handler must NOT have fired.
    expect(onProviderModelChange).not.toHaveBeenCalled();
    // Picker must still be open (selection would have closed it).
    expect(
      screen.getByPlaceholderText("Search models..."),
    ).toBeInTheDocument();
  });

  it("favorited models bubble to the top of the rail's model list", async () => {
    const user = userEvent.setup();
    // Favorite the SECOND row (Haiku) so its insertion-order spot is
    // the bottom; the favorites sort must lift it above Opus.
    usePickerFavorites
      .getState()
      .toggle("claude", "claude-haiku-4-5");
    renderPicker();
    await openPicker(user);

    const rows = screen.getAllByTestId("multi-provider-model-row");
    expect(rows[0]?.textContent).toContain("Claude Haiku 4.5");
    expect(rows[1]?.textContent).toContain("Claude Opus 4.7");
  });

  it("favorites bubble to the top of search results across providers", async () => {
    const user = userEvent.setup();
    // Cross-provider favorites: a Codex one + an OpenCode one. A
    // generic search ("5") matches all of {GPT-5.4, GPT-5.4 Mini,
    // GPT-5.3 Codex, GPT-5, Sonnet 4.6, Haiku 4.5}; the favorites
    // must surface ABOVE the non-favorites.
    usePickerFavorites.getState().toggle("codex", "gpt-5.3-codex");
    usePickerFavorites.getState().toggle("opencode", "openai/gpt-5");
    renderPicker();
    await openPicker(user);

    await user.type(screen.getByPlaceholderText("Search models..."), "5");

    await waitFor(() => {
      const rows = screen.getAllByTestId("multi-provider-model-row");
      const order = rows.map((r) => r.textContent ?? "");
      // First two rows should be the favorites — order between them
      // is undefined (insertion order, not provider rail order), so
      // assert membership not exact slot.
      const top2 = order.slice(0, 2).join("|");
      expect(top2).toMatch(/GPT-5.3 Codex|GPT-5\b/);
      expect(top2.match(/GPT-5\b/)).not.toBeNull();
      expect(top2.match(/GPT-5.3 Codex/)).not.toBeNull();
      // A non-favorite that also matches the query must come AFTER
      // both favorites. Haiku 4.5 matches the "5" query and is not
      // favorited; pin it as a non-favorite floor.
      const haikuIdx = order.findIndex((t) =>
        t.includes("Claude Haiku 4.5"),
      );
      if (haikuIdx >= 0) {
        expect(haikuIdx).toBeGreaterThan(1);
      }
    });
  });

  it("unfavoriting flips the star empty and re-sorts", async () => {
    const user = userEvent.setup();
    usePickerFavorites
      .getState()
      .toggle("claude", "claude-haiku-4-5");
    renderPicker();
    await openPicker(user);

    // Initial state: Haiku at top.
    let rows = screen.getAllByTestId("multi-provider-model-row");
    expect(rows[0]?.textContent).toContain("Claude Haiku 4.5");

    // Click the star on the (now-favorited) top row.
    const haikuStar = rows[0]!.querySelector(
      '[data-testid="model-row-favorite-toggle"]',
    ) as HTMLElement;
    await user.click(haikuStar);

    // Sort should rerun: Opus moves to top, Haiku to bottom.
    await waitFor(() => {
      rows = screen.getAllByTestId("multi-provider-model-row");
      expect(rows[0]?.textContent).toContain("Claude Opus 4.7");
      expect(rows[1]?.textContent).toContain("Claude Haiku 4.5");
    });
    expect(
      usePickerFavorites.getState().isFavorite("claude", "claude-haiku-4-5"),
    ).toBe(false);
  });

  it("favorites tab is hidden when no favorites are set", async () => {
    const user = userEvent.setup();
    expect(usePickerFavorites.getState().favorites).toEqual([]);
    renderPicker();
    await openPicker(user);
    expect(
      screen.queryByTestId("provider-rail-favorites"),
    ).not.toBeInTheDocument();
  });

  it("favorites tab appears once a favorite exists and shows cross-driver favorited rows", async () => {
    const user = userEvent.setup();
    // Cross-driver: one Claude favorite + one OpenCode favorite. The
    // favorites tab should bundle both regardless of which rail is
    // active.
    usePickerFavorites
      .getState()
      .toggle("claude", "claude-haiku-4-5");
    usePickerFavorites
      .getState()
      .toggle("opencode", "openrouter/x-ai/grok-2");
    renderPicker({ provider: "codex", model: "gpt-5.4" });
    await openPicker(user);

    // Tab is now rendered.
    const favTab = screen.getByTestId("provider-rail-favorites");
    expect(favTab).toBeInTheDocument();

    await user.click(favTab);
    await waitFor(() => {
      const rows = screen.getAllByTestId("multi-provider-model-row");
      const labels = rows.map((r) => r.textContent ?? "");
      // Both favorited rows surface; non-favorited rows do not.
      expect(labels.some((l) => l.includes("Claude Haiku 4.5"))).toBe(true);
      expect(labels.some((l) => l.includes("Grok 2"))).toBe(true);
      expect(labels.some((l) => l.includes("GPT-5.4 (Codex)"))).toBe(false);
      expect(labels.some((l) => l.includes("Claude Opus 4.7"))).toBe(false);
    });
  });

  it("renders a FREE badge on free-tier rows and sorts them above paid ones", async () => {
    // Reseed the OpenCode caps so two of three rows are free-tier.
    // Sort behaviour:
    //   1. favorites first (none here)
    //   2. free models bubble above paid
    //   3. insertion order preserved within each tier
    const FREE_OPENCODE_CAPS: ProviderChatCapabilities = {
      ...OPENCODE_CAPS,
      models: [
        // Insertion order: paid, free, paid, free. After sort the
        // two free rows must precede the two paid rows.
        makeModel({
          id: "openai/gpt-5",
          label: "GPT-5",
          sub_provider: "openai",
          is_free: false,
        }),
        makeModel({
          id: "openrouter/x-ai/grok-2",
          label: "Grok 2",
          sub_provider: "openrouter",
          is_free: true,
        }),
        makeModel({
          id: "anthropic/claude-sonnet-4-6",
          label: "Claude Sonnet 4.6",
          sub_provider: "anthropic",
          is_free: false,
        }),
        makeModel({
          id: "venice/llama-3.3",
          label: "Llama 3.3",
          sub_provider: "venice",
          is_free: true,
        }),
      ],
    };
    seedStore({
      claude: CLAUDE_CAPS,
      codex: CODEX_CAPS,
      opencode: FREE_OPENCODE_CAPS,
    });

    const user = userEvent.setup();
    renderPicker();
    await openPicker(user);
    await user.click(screen.getByTestId("provider-rail-opencode"));

    const rows = await screen.findAllByTestId("multi-provider-model-row");
    const labels = rows.map((r) => r.textContent ?? "");
    // Free rows occupy the top two slots regardless of insertion
    // order.
    expect(labels[0]).toMatch(/Grok 2|Llama 3\.3/);
    expect(labels[1]).toMatch(/Grok 2|Llama 3\.3/);
    // Paid rows follow.
    expect(labels[2]).not.toMatch(/Grok 2|Llama 3\.3/);

    // FREE badges render only on the two free rows.
    const badges = screen.getAllByTestId("model-row-free-badge");
    expect(badges).toHaveLength(2);
  });

  it("favorites still beat free-tier in sort order", async () => {
    // A favorited paid model must outrank a non-favorited free
    // model — the favorites tier is the strongest sort signal.
    const MIXED_OPENCODE_CAPS: ProviderChatCapabilities = {
      ...OPENCODE_CAPS,
      models: [
        makeModel({
          id: "openai/gpt-5",
          label: "GPT-5",
          sub_provider: "openai",
          is_free: false,
        }),
        makeModel({
          id: "venice/llama-3.3",
          label: "Llama 3.3",
          sub_provider: "venice",
          is_free: true,
        }),
      ],
    };
    seedStore({
      claude: CLAUDE_CAPS,
      codex: CODEX_CAPS,
      opencode: MIXED_OPENCODE_CAPS,
    });
    usePickerFavorites.getState().toggle("opencode", "openai/gpt-5");

    const user = userEvent.setup();
    renderPicker();
    await openPicker(user);
    await user.click(screen.getByTestId("provider-rail-opencode"));

    const rows = await screen.findAllByTestId("multi-provider-model-row");
    expect(rows[0]?.textContent).toContain("GPT-5"); // favorited paid
    expect(rows[1]?.textContent).toContain("Llama 3.3"); // free, no favorite
  });

  it("empty-favorites default does not break rendering", async () => {
    const user = userEvent.setup();
    // Confidence check: with zero favorites in the store, every
    // existing test surface still renders cleanly. (The earlier
    // tests in this file already prove this implicitly; pin it.)
    expect(usePickerFavorites.getState().favorites).toEqual([]);
    renderPicker();
    await openPicker(user);
    expect(
      screen.getByPlaceholderText("Search models..."),
    ).toBeInTheDocument();
    const rows = screen.getAllByTestId("multi-provider-model-row");
    expect(rows.length).toBeGreaterThan(0);
  });
});
