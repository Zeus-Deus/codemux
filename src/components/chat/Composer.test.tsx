/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";
import type { ComponentProps } from "react";
import { TooltipProvider } from "@/components/ui/tooltip";

// Provider slash-command discovery runs on every popup open. Mock the
// IPC so the COMMANDS group resolves empty instead of hitting the
// unmocked Tauri bridge (which rejects and leaves a sticky error entry
// in the shared store that can bleed across tests).
vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return {
    ...actual,
    listChatSlashCommands: vi.fn(),
  };
});

import { Composer } from "./Composer";
import { listChatSlashCommands } from "@/tauri/commands";
import { useProviderCommandsStore } from "@/stores/provider-commands-store";
import type { ChatModelInfo } from "@/tauri/types";
import type { Attachment } from "@/stores/agent-chat-store";

type ComposerProps = ComponentProps<typeof Composer>;

const listChatSlashCommandsMock =
  listChatSlashCommands as unknown as ReturnType<typeof vi.fn>;

beforeEach(() => {
  // Reset the shared provider-commands cache + IPC mock so each test
  // starts from a clean, empty-resolving COMMANDS group.
  useProviderCommandsStore.getState().invalidate();
  listChatSlashCommandsMock.mockReset();
  listChatSlashCommandsMock.mockResolvedValue([]);
});

afterEach(() => cleanup());

function baseProps(): ComposerProps {
  return {
    draft: "",
    cwd: "/home/user",
    provider: "claude",
    model: null,
    permissionMode: null,
    effort: null,
    contextWindow: null,
    activeModel: null,
    effortLabelMap: {},
    permissionModes: null,
    ultrathinkInBodyText: false,
    streaming: false,
    sessionReady: true,
    showProviderPicker: false,
    mode: "default",
    onDraftChange: vi.fn(),
    onSubmit: vi.fn(),
    onStop: vi.fn(),
    onProviderModelChange: vi.fn(),
    onModelChange: vi.fn(),
    onPermissionModeChange: vi.fn(),
    onEffortChange: vi.fn(),
    onContextWindowChange: vi.fn(),
    onModeActivate: vi.fn(),
    onModeRemove: vi.fn(),
  };
}

function renderComposer(props: Partial<ComposerProps> = {}) {
  return render(
    <TooltipProvider>
      <Composer {...baseProps()} {...props} />
    </TooltipProvider>,
  );
}

describe("Composer", () => {
  describe("error banner (§8)", () => {
    it("renders the banner when errorMessage is set", () => {
      const { getByRole } = renderComposer({
        errorMessage: "workspace creation failed",
      });
      const alert = getByRole("alert");
      expect(alert.textContent).toContain("Send failed");
      expect(alert.textContent).toContain("workspace creation failed");
      expect(alert.textContent).toContain("Press Enter to retry");
    });

    it("does not render the banner when errorMessage is null", () => {
      const { queryByRole } = renderComposer({ errorMessage: null });
      expect(queryByRole("alert")).toBeNull();
    });

    it("does not render the banner when errorMessage is undefined (omitted)", () => {
      const { queryByRole } = renderComposer();
      expect(queryByRole("alert")).toBeNull();
    });
  });

  describe("showStopButton (§6.5)", () => {
    it("hides the Stop button while streaming when showStopButton=false", () => {
      const { container } = renderComposer({
        streaming: true,
        showStopButton: false,
      });
      expect(container.querySelector('button[aria-label="Stop"]')).toBeNull();
      // The Send button remains visible but disabled, so the draft
      // composer still reads as 'working…' to the user.
      const send = container.querySelector(
        'button[aria-label="Send"]',
      ) as HTMLButtonElement | null;
      expect(send).not.toBeNull();
      expect(send!.disabled).toBe(true);
    });

    it("renders the Stop button while streaming by default", () => {
      const { container } = renderComposer({ streaming: true });
      expect(container.querySelector('button[aria-label="Stop"]')).not.toBeNull();
    });
  });

  describe("zone1Override (§12)", () => {
    it("replaces the default cwd strip when provided", () => {
      const { container, queryByText } = renderComposer({
        zone1Override: <button aria-label="custom-picker">Pick a project</button>,
        cwd: "/home/user/projects/foo",
      });
      // Default cwd label hidden.
      expect(queryByText("/home/user/projects/foo")).toBeNull();
      // Override rendered in the Zone 1 slot.
      expect(
        container.querySelector('button[aria-label="custom-picker"]'),
      ).not.toBeNull();
    });

    it("falls back to the cwd label when no override is provided", () => {
      const { getByText } = renderComposer({
        cwd: "/home/user/projects/foo",
      });
      expect(getByText("/home/user/projects/foo")).toBeInTheDocument();
    });

    it("hides the cwd strip when both override and cwd are absent", () => {
      const { container } = renderComposer({ cwd: null });
      // Nothing rendered above the textarea.
      expect(container.textContent).not.toContain("Home");
    });

    it("renders nothing above the textarea when the override is null (running chat)", () => {
      const { queryByText } = renderComposer({
        zone1Override: null,
        cwd: "/home/user/projects/foo",
      });
      // `null` suppresses the default cwd label — a running chat keeps
      // its scope in the workspace context bar instead.
      expect(queryByText("/home/user/projects/foo")).toBeNull();
    });
  });

  describe("belowComposerSlot (Thread Scope redesign)", () => {
    it("renders the slot below the composer card when provided", () => {
      const { container, getByText } = renderComposer({
        belowComposerSlot: <div>scope row here</div>,
      });
      expect(getByText("scope row here")).toBeInTheDocument();
      // Below the composer-wrapper card in DOM order, not above it.
      const wrapper = container.querySelector(
        '[data-testid="composer-wrapper"]',
      )!;
      const slot = getByText("scope row here");
      expect(wrapper.compareDocumentPosition(slot)).toBe(
        Node.DOCUMENT_POSITION_FOLLOWING,
      );
    });

    it("renders nothing extra when belowComposerSlot is omitted", () => {
      const { container } = renderComposer();
      const wrapper = container.querySelector(
        '[data-testid="composer-wrapper"]',
      )!;
      expect(wrapper.nextElementSibling).toBeNull();
    });
  });

  describe("staged attachment chip strip (Step 8 Stage 1 — image-only post-2.1)", () => {
    // Stage 2.1 moved file/folder chips inside the textarea (rendered
    // by the mirror overlay). The above-textarea strip is now
    // reserved for image attachments, which can't live inline as
    // text. These tests use image kind so the strip mechanism stays
    // covered for Stage 6.
    function makeImageAttachment(overrides: Partial<Attachment> = {}): Attachment {
      return {
        id: "img-1",
        kind: "image",
        ref: "image:1",
        metadata: { label: "screenshot.png" },
        ...overrides,
      };
    }

    it("does not render the strip when stagedAttachments is empty", () => {
      const { queryByTestId } = renderComposer();
      expect(queryByTestId("composer-attachment-strip")).toBeNull();
    });

    it("does not render the strip when only file/folder kinds are staged (those go inline)", () => {
      const { queryByTestId } = renderComposer({
        stagedAttachments: [
          {
            id: "a",
            kind: "file",
            ref: "/repo/Composer.tsx",
            metadata: { label: "Composer.tsx" },
          },
        ],
      });
      expect(queryByTestId("composer-attachment-strip")).toBeNull();
    });

    it("renders one chip per image attachment in the strip", () => {
      const { getByTestId, getByText } = renderComposer({
        stagedAttachments: [
          makeImageAttachment({ id: "a", metadata: { label: "a.png" } }),
          makeImageAttachment({ id: "b", metadata: { label: "b.png" } }),
          makeImageAttachment({ id: "c", metadata: { label: "c.png" } }),
        ],
      });
      const strip = getByTestId("composer-attachment-strip");
      expect(strip).toBeInTheDocument();
      expect(getByText("a.png")).toBeInTheDocument();
      expect(getByText("b.png")).toBeInTheDocument();
      expect(getByText("c.png")).toBeInTheDocument();
    });

    it("file attachments are NOT rendered in the strip even if interleaved with images", () => {
      const { getByTestId, getByText, queryByText } = renderComposer({
        stagedAttachments: [
          {
            id: "f",
            kind: "file",
            ref: "/repo/Composer.tsx",
            metadata: { label: "Composer.tsx" },
          },
          makeImageAttachment({ id: "i", metadata: { label: "shot.png" } }),
        ],
      });
      const strip = getByTestId("composer-attachment-strip");
      expect(strip).toBeInTheDocument();
      expect(getByText("shot.png")).toBeInTheDocument();
      // The file's basename never reaches the strip. The mirror
      // shows it inline only when the matching `@<basename>` token
      // is in the textarea — empty draft means no chip anywhere.
      expect(queryByText("Composer.tsx")).toBeNull();
    });

    it("calls onRemoveAttachment with the chip id when X is clicked on an image chip", () => {
      const onRemoveAttachment = vi.fn();
      const { getByLabelText } = renderComposer({
        stagedAttachments: [makeImageAttachment()],
        onRemoveAttachment,
      });
      fireEvent.click(getByLabelText("Remove screenshot.png"));
      expect(onRemoveAttachment).toHaveBeenCalledWith("img-1");
    });

    it("the strip wraps gracefully (chip strip uses flex-wrap)", () => {
      const { getByTestId } = renderComposer({
        stagedAttachments: Array.from({ length: 8 }).map((_, i) =>
          makeImageAttachment({ id: `img-${i}`, metadata: { label: `i${i}.png` } }),
        ),
      });
      const strip = getByTestId("composer-attachment-strip");
      expect(strip.className).toContain("flex-wrap");
    });
  });

  describe("mode pill above textarea (Step 8 Stage 3 refactor)", () => {
    // Stage 3 retired the `+ Mode` dropdown; the active mode chip
    // moved out of the footer and now lives in the strip above the
    // textarea, alongside any image attachment chips.
    it("renders the strip with a Plan mode pill when mode is plan", () => {
      const { getByTestId, getByRole } = renderComposer({ mode: "plan" });
      expect(getByTestId("composer-attachment-strip")).toBeInTheDocument();
      expect(
        getByRole("status", { name: /Plan mode active/i }),
      ).toBeInTheDocument();
    });

    it("renders no strip when mode is default and no image attachments are staged", () => {
      const { queryByTestId } = renderComposer({ mode: "default" });
      expect(queryByTestId("composer-attachment-strip")).toBeNull();
    });

    it("the mode pill renders before image attachment chips in the strip", () => {
      const { getByTestId } = renderComposer({
        mode: "ask",
        stagedAttachments: [
          {
            id: "img-1",
            kind: "image",
            ref: "image:1",
            metadata: { label: "shot.png" },
          },
        ],
      });
      const strip = getByTestId("composer-attachment-strip");
      const text = strip.textContent ?? "";
      const askIdx = text.indexOf("Ask");
      const imgIdx = text.indexOf("shot.png");
      expect(askIdx).toBeGreaterThanOrEqual(0);
      expect(imgIdx).toBeGreaterThan(askIdx);
    });

    it("clicking X on the mode pill calls onModeRemove", () => {
      const onModeRemove = vi.fn();
      const { getByLabelText } = renderComposer({
        mode: "debug",
        onModeRemove,
      });
      fireEvent.click(getByLabelText(/Remove Debug mode/i));
      expect(onModeRemove).toHaveBeenCalled();
    });
  });

  describe("inline attachment chip in the mirror (Step 8 Stage 2.1)", () => {
    // The mirror overlay paints a chip-style background on
    // `@<basename>` tokens whose basename matches a staged
    // file/folder attachment. Verifies the token is recognised and
    // the chip's loading/error state classes apply.
    it("does not paint a chip when the token has no matching attachment", () => {
      const { container } = renderComposer({ draft: "@unknown.ts" });
      // A bare `@unknown.ts` with no slice match falls through as
      // plain prose — no chip span.
      expect(
        container.querySelector('[data-testid^="composer-attachment-token-"]'),
      ).toBeNull();
    });

    it("paints a chip on @<basename> when the staged attachment matches", () => {
      const { getByTestId } = renderComposer({
        draft: "look at @Composer.tsx now",
        stagedAttachments: [
          {
            id: "f",
            kind: "file",
            ref: "/repo/src/components/chat/Composer.tsx",
            metadata: { label: "Composer.tsx" },
          },
        ],
      });
      const chip = getByTestId("composer-attachment-token-Composer.tsx");
      expect(chip).toBeInTheDocument();
      expect(chip.className).toContain("bg-foreground/10");
      expect(chip.textContent).toBe("@Composer.tsx");
    });

    it("dims the chip when the attachment is loading", () => {
      const { getByTestId } = renderComposer({
        draft: "look at @Composer.tsx",
        stagedAttachments: [
          {
            id: "f",
            kind: "file",
            ref: "/repo/Composer.tsx",
            metadata: { label: "Composer.tsx", isLoading: true },
          },
        ],
      });
      const chip = getByTestId("composer-attachment-token-Composer.tsx");
      expect(chip.className).toContain("opacity-60");
      expect(chip.getAttribute("data-loading")).toBe("true");
    });

    it("renders a destructive chip when the attachment has an error", () => {
      const { getByTestId } = renderComposer({
        draft: "look at @Composer.tsx",
        stagedAttachments: [
          {
            id: "f",
            kind: "file",
            ref: "/repo/Composer.tsx",
            metadata: {
              label: "Composer.tsx",
              error: "permission denied",
            },
          },
        ],
      });
      const chip = getByTestId("composer-attachment-token-Composer.tsx");
      expect(chip.className).toContain("text-destructive");
      expect(chip.getAttribute("data-error")).toBe("true");
    });
  });

  describe("post-materialize picker visibility (Stage C Effort-lock fix)", () => {
    // A capability payload mimicking what `provider-capabilities-store`
    // returns for Claude Opus 4.7 after Rust hydration — the draft
    // materialize flow seeds this onto the slice.
    const fullActiveModel: ChatModelInfo = {
      id: "claude-opus-4-7",
      label: "Claude Opus 4.7",
      description: "Strongest Claude model",
      effort_levels: ["low", "medium", "high", "xhigh", "max"],
      default_effort: "xhigh",
      prompt_injected_effort_levels: ["ultrathink"],
      // Highest-is-default rule: 1M is flagged default in
      // production. With always-render the picker shows up either
      // way, so the test below deliberately exercises a non-default
      // selection to keep assertions discriminating.
      context_window_options: [
        { value: "200k", label: "200k", is_default: false },
        { value: "1m", label: "1M", is_default: true },
      ],
      supports_adaptive_thinking: true,
      supports_thinking_toggle: false,
      supports_fast_mode: false,
      supports_images: true,
      sub_provider: null,
      is_free: false,
    };

    it("renders ReasoningPicker when activeModel is populated (not null)", () => {
      const { container } = renderComposer({
        model: "claude-opus-4-7",
        effort: "xhigh",
        activeModel: fullActiveModel,
        effortLabelMap: {
          low: "Low",
          medium: "Medium",
          high: "High",
          xhigh: "Extra High",
          max: "Max",
          ultrathink: "Ultrathink",
        },
      });
      const buttons = Array.from(
        container.querySelectorAll<HTMLButtonElement>("button"),
      );
      // ReasoningPicker's trigger shows the combined "Effort · Context"
      // label ("Extra High · 1M" here — contextWindow falls back to
      // the model's is_default via Option C). Presence of that text is
      // proof the picker rendered rather than returning null.
      const reasoningTrigger = buttons.find((b) =>
        b.textContent?.includes("Extra High"),
      );
      expect(reasoningTrigger).toBeDefined();
      expect(reasoningTrigger?.disabled).toBe(false);
    });

    it("renders the context-window label in the ReasoningPicker pill, even on the default", () => {
      // Always-render rule (Stage C follow-up): the pill shows up
      // whether the user is on the default (1M) or on a non-default
      // option. Verify both.
      const onDefault = renderComposer({
        model: "claude-opus-4-7",
        contextWindow: "1m",
        activeModel: fullActiveModel,
      });
      expect(onDefault.container.textContent).toContain("1M");
      cleanup();
      const onNonDefault = renderComposer({
        model: "claude-opus-4-7",
        contextWindow: "200k",
        activeModel: fullActiveModel,
      });
      expect(onNonDefault.container.textContent).toContain("200k");
    });

    it("hides both pickers when activeModel is null (regression guard)", () => {
      const { container } = renderComposer({
        model: null,
        activeModel: null,
      });
      // Neither "Extra High" nor "200k" should appear anywhere.
      expect(container.textContent).not.toContain("Extra High");
      expect(container.textContent).not.toContain("200k");
    });
  });

  describe("Stage 3 — mode pill integration", () => {
    it("swaps the textarea placeholder to match the active mode", () => {
      const { getByPlaceholderText } = renderComposer({
        mode: "plan",
        sessionReady: true,
      });
      expect(
        getByPlaceholderText("Plan and design before coding…"),
      ).toBeInTheDocument();
    });

    it("uses the live-session default placeholder when mode is default", () => {
      const { getByPlaceholderText } = renderComposer({
        mode: "default",
        sessionReady: true,
      });
      expect(
        getByPlaceholderText("Reply or steer the agent…"),
      ).toBeInTheDocument();
    });

    it("uses the draft-variant default placeholder when isDraft is set", () => {
      const { getByPlaceholderText } = renderComposer({
        mode: "default",
        sessionReady: true,
        isDraft: true,
      });
      expect(
        getByPlaceholderText("Describe what you want the agent to do…"),
      ).toBeInTheDocument();
    });

    // Stage 8 replaces the auto-activate-on-typing flow with a popup.
    // See the "Stage 8 — slash command popup" describe block below.
  });

  describe("Stage 8 — slash command popup", () => {
    /**
     * Helper: simulate the user typing into the textarea. Sets the
     * native value and fires an `input` event so React's controlled-
     * input handlers run, plus updates `selectionStart` so the
     * cursor-aware slash detection can find the slash.
     */
    function type(textarea: HTMLTextAreaElement, value: string, cursor?: number) {
      const setter = Object.getOwnPropertyDescriptor(
        window.HTMLTextAreaElement.prototype,
        "value",
      )?.set;
      setter?.call(textarea, value);
      const cur = cursor ?? value.length;
      textarea.setSelectionRange(cur, cur);
      textarea.dispatchEvent(new Event("input", { bubbles: true }));
    }

    function getTextarea(container: HTMLElement) {
      return container.querySelector("textarea") as HTMLTextAreaElement;
    }

    it("typing '/' at the start opens the popup with all three modes", () => {
      const { container, queryByTestId } = renderComposer({ mode: "default" });
      expect(queryByTestId("slash-command-popup")).toBeNull();
      type(getTextarea(container), "/");
      expect(queryByTestId("slash-command-popup")).not.toBeNull();
      expect(queryByTestId("slash-item-mode:plan")).not.toBeNull();
      expect(queryByTestId("slash-item-mode:ask")).not.toBeNull();
      expect(queryByTestId("slash-item-mode:debug")).not.toBeNull();
    });

    it("filters items as the user types (/pl → only Plan)", () => {
      const { container, queryByTestId } = renderComposer({ mode: "default" });
      const textarea = getTextarea(container);
      type(textarea, "/pl");
      expect(queryByTestId("slash-item-mode:plan")).not.toBeNull();
      expect(queryByTestId("slash-item-mode:ask")).toBeNull();
      expect(queryByTestId("slash-item-mode:debug")).toBeNull();
    });

    it("opens the popup when '/' is typed mid-prose after a space", () => {
      const { container, queryByTestId } = renderComposer({
        mode: "default",
        draft: "hello ",
      });
      const textarea = getTextarea(container);
      type(textarea, "hello /pl");
      expect(queryByTestId("slash-command-popup")).not.toBeNull();
      expect(queryByTestId("slash-item-mode:plan")).not.toBeNull();
    });

    it("does NOT open when '/' is inside a word", () => {
      const { container, queryByTestId } = renderComposer({ mode: "default" });
      const textarea = getTextarea(container);
      type(textarea, "a/b");
      expect(queryByTestId("slash-command-popup")).toBeNull();
    });

    it("Enter on a highlighted item activates the mode and strips the slash text", () => {
      const onDraftChange = vi.fn();
      const onModeActivate = vi.fn();
      const { container } = renderComposer({
        mode: "default",
        onDraftChange,
        onModeActivate,
      });
      const textarea = getTextarea(container);
      type(textarea, "/pl");
      // After filter, only Plan is visible — auto-highlighted by the
      // useEffect that initialises highlight to the first visible item.
      fireEvent.keyDown(textarea, { key: "Enter" });
      expect(onModeActivate).toHaveBeenCalledWith("plan");
      // Composer asks the parent to drop the typed `/pl` from the draft.
      expect(onDraftChange).toHaveBeenLastCalledWith("");
    });

    it("Enter mid-prose preserves text before the slash", () => {
      const onDraftChange = vi.fn();
      const onModeActivate = vi.fn();
      const { container } = renderComposer({
        mode: "default",
        draft: "hello ",
        onDraftChange,
        onModeActivate,
      });
      const textarea = getTextarea(container);
      type(textarea, "hello /pl");
      fireEvent.keyDown(textarea, { key: "Enter" });
      expect(onModeActivate).toHaveBeenCalledWith("plan");
      // Last call from the picker handler — the text before the slash
      // is preserved verbatim, the slash and "pl" are stripped.
      expect(onDraftChange).toHaveBeenLastCalledWith("hello ");
    });

    it("Esc closes the popup, and the slash is reported back to the parent (preserved)", () => {
      const onDraftChange = vi.fn();
      const onModeActivate = vi.fn();
      const { container, queryByTestId } = renderComposer({
        mode: "default",
        onDraftChange,
        onModeActivate,
      });
      const textarea = getTextarea(container);
      type(textarea, "/");
      expect(queryByTestId("slash-command-popup")).not.toBeNull();
      // The slash made it through to the parent — typing always reports
      // the literal value; the popup is purely a UI layer.
      expect(onDraftChange).toHaveBeenCalledWith("/");

      fireEvent.keyDown(textarea, { key: "Escape" });
      expect(queryByTestId("slash-command-popup")).toBeNull();
      // No mode activation happened, so the user's `/` survives in
      // whatever state the parent decides to render.
      expect(onModeActivate).not.toHaveBeenCalled();
    });

    it("Arrow keys move the highlight without leaving the textarea", () => {
      const { container, getByTestId } = renderComposer({ mode: "default" });
      const textarea = getTextarea(container);
      type(textarea, "/");
      // First visible item is highlighted by default.
      expect(getByTestId("slash-item-mode:plan")).toHaveAttribute(
        "data-selected",
        "true",
      );
      fireEvent.keyDown(textarea, { key: "ArrowDown" });
      expect(getByTestId("slash-item-mode:ask")).toHaveAttribute(
        "data-selected",
        "true",
      );
      fireEvent.keyDown(textarea, { key: "ArrowDown" });
      expect(getByTestId("slash-item-mode:debug")).toHaveAttribute(
        "data-selected",
        "true",
      );
      // The WORKFLOWS group's single `/workflow` row follows MODES.
      fireEvent.keyDown(textarea, { key: "ArrowDown" });
      expect(getByTestId("slash-item-workflow")).toHaveAttribute(
        "data-selected",
        "true",
      );
      // The SETTINGS group's single `/model` row follows WORKFLOWS.
      fireEvent.keyDown(textarea, { key: "ArrowDown" });
      expect(getByTestId("slash-item-composer:model")).toHaveAttribute(
        "data-selected",
        "true",
      );
      fireEvent.keyDown(textarea, { key: "ArrowDown" });
      // Wraps around to the top.
      expect(getByTestId("slash-item-mode:plan")).toHaveAttribute(
        "data-selected",
        "true",
      );
      fireEvent.keyDown(textarea, { key: "ArrowUp" });
      // Wraps from top to bottom, landing on the last item (/model).
      expect(getByTestId("slash-item-composer:model")).toHaveAttribute(
        "data-selected",
        "true",
      );
    });

    it("hides the active mode from the popup items", () => {
      const { container, queryByTestId } = renderComposer({ mode: "plan" });
      const textarea = getTextarea(container);
      type(textarea, "/");
      expect(queryByTestId("slash-item-mode:plan")).toBeNull();
      expect(queryByTestId("slash-item-mode:ask")).not.toBeNull();
      expect(queryByTestId("slash-item-mode:debug")).not.toBeNull();
    });

    /**
     * Regression for the user-reported bug where the popup auto-closed
     * once the user typed past the slash. Mirrors a real-world typing
     * pattern: each character arrives in its own input event.
     */
    it("popup stays open through incremental typing of '/', '/p', '/pl'", () => {
      const { container, queryByTestId } = renderComposer({ mode: "default" });
      const textarea = getTextarea(container);
      type(textarea, "/");
      expect(queryByTestId("slash-command-popup")).not.toBeNull();
      type(textarea, "/p");
      expect(queryByTestId("slash-command-popup")).not.toBeNull();
      // After "/p" the filter narrows to Plan only.
      expect(queryByTestId("slash-item-mode:plan")).not.toBeNull();
      expect(queryByTestId("slash-item-mode:ask")).toBeNull();
      type(textarea, "/pl");
      expect(queryByTestId("slash-command-popup")).not.toBeNull();
      expect(queryByTestId("slash-item-mode:plan")).not.toBeNull();
      expect(queryByTestId("slash-item-mode:ask")).toBeNull();
      expect(queryByTestId("slash-item-mode:debug")).toBeNull();
    });

    it("typing /de filters to Debug only and keeps the popup open", () => {
      const { container, queryByTestId } = renderComposer({ mode: "default" });
      const textarea = getTextarea(container);
      type(textarea, "/de");
      expect(queryByTestId("slash-command-popup")).not.toBeNull();
      expect(queryByTestId("slash-item-mode:debug")).not.toBeNull();
      expect(queryByTestId("slash-item-mode:plan")).toBeNull();
      expect(queryByTestId("slash-item-mode:ask")).toBeNull();
    });

    it("typing /zzz keeps the popup open with the empty-state message", () => {
      const { container, queryByTestId, getByText } = renderComposer({
        mode: "default",
      });
      const textarea = getTextarea(container);
      type(textarea, "/zzz");
      // Popup stays mounted to surface "No commands match" instead of
      // silently disappearing.
      expect(queryByTestId("slash-command-popup")).not.toBeNull();
      expect(getByText(/No commands match/i)).toBeInTheDocument();
      expect(queryByTestId("slash-item-mode:plan")).toBeNull();
    });

    it("Enter falls through to submit when popup is empty (no items to pick)", () => {
      const onSubmit = vi.fn();
      const onModeActivate = vi.fn();
      const { container } = renderComposer({
        mode: "default",
        draft: "/zzz",
        onSubmit,
        onModeActivate,
      });
      const textarea = getTextarea(container);
      // Simulate the user being parked at "/zzz" with the empty-state
      // popup visible. Pressing Enter should *send* the message —
      // there's no item to activate.
      type(textarea, "/zzz");
      fireEvent.keyDown(textarea, { key: "Enter" });
      expect(onModeActivate).not.toHaveBeenCalled();
      expect(onSubmit).toHaveBeenCalled();
    });
  });

  describe("Stage 8 — Shift+Tab mode cycling", () => {
    function getTextarea(container: HTMLElement) {
      return container.querySelector("textarea") as HTMLTextAreaElement;
    }

    it("cycles default → plan", () => {
      const onModeActivate = vi.fn();
      const onModeRemove = vi.fn();
      const { container } = renderComposer({
        mode: "default",
        onModeActivate,
        onModeRemove,
      });
      fireEvent.keyDown(getTextarea(container), { key: "Tab", shiftKey: true });
      expect(onModeActivate).toHaveBeenCalledWith("plan");
      expect(onModeRemove).not.toHaveBeenCalled();
    });

    it("cycles plan → ask", () => {
      const onModeActivate = vi.fn();
      const { container } = renderComposer({
        mode: "plan",
        onModeActivate,
      });
      fireEvent.keyDown(getTextarea(container), { key: "Tab", shiftKey: true });
      expect(onModeActivate).toHaveBeenCalledWith("ask");
    });

    it("cycles ask → debug", () => {
      const onModeActivate = vi.fn();
      const { container } = renderComposer({
        mode: "ask",
        onModeActivate,
      });
      fireEvent.keyDown(getTextarea(container), { key: "Tab", shiftKey: true });
      expect(onModeActivate).toHaveBeenCalledWith("debug");
    });

    it("cycles debug → default (calls onModeRemove, not onModeActivate)", () => {
      const onModeActivate = vi.fn();
      const onModeRemove = vi.fn();
      const { container } = renderComposer({
        mode: "debug",
        onModeActivate,
        onModeRemove,
      });
      fireEvent.keyDown(getTextarea(container), { key: "Tab", shiftKey: true });
      expect(onModeRemove).toHaveBeenCalled();
      expect(onModeActivate).not.toHaveBeenCalled();
    });

    it("preventDefault is called on the keydown so native focus-tab nav doesn't run", () => {
      const { container } = renderComposer({ mode: "default" });
      const textarea = getTextarea(container);
      const event = new KeyboardEvent("keydown", {
        key: "Tab",
        shiftKey: true,
        bubbles: true,
        cancelable: true,
      });
      textarea.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(true);
    });
  });

  describe("Stage 8 — submit flow regression", () => {
    function getTextarea(container: HTMLElement) {
      return container.querySelector("textarea") as HTMLTextAreaElement;
    }

    it("Enter still submits when popup is closed", () => {
      const onSubmit = vi.fn();
      const { container } = renderComposer({
        mode: "default",
        draft: "hello",
        onSubmit,
      });
      fireEvent.keyDown(getTextarea(container), { key: "Enter" });
      expect(onSubmit).toHaveBeenCalled();
    });

    it("Shift+Enter does NOT submit (newline path preserved)", () => {
      const onSubmit = vi.fn();
      const { container } = renderComposer({
        mode: "default",
        draft: "hello",
        onSubmit,
      });
      fireEvent.keyDown(getTextarea(container), {
        key: "Enter",
        shiftKey: true,
      });
      expect(onSubmit).not.toHaveBeenCalled();
    });
  });

  describe("Continue-run chip (issue #154)", () => {
    const CHIP = "composer-continue-run-chip";

    it("renders when interrupted with nothing in flight and a handler wired", () => {
      const { getByTestId } = renderComposer({
        interrupted: true,
        onContinueRun: vi.fn(),
      });
      const chip = getByTestId(CHIP);
      expect(chip).toHaveTextContent("Continue run");
    });

    it("clicking the chip calls onContinueRun exactly once", () => {
      const onContinueRun = vi.fn();
      const { getByTestId } = renderComposer({
        interrupted: true,
        onContinueRun,
      });
      fireEvent.click(getByTestId(CHIP));
      expect(onContinueRun).toHaveBeenCalledTimes(1);
    });

    it("does not render while streaming (a live turn owns the composer)", () => {
      const { queryByTestId } = renderComposer({
        interrupted: true,
        streaming: true,
        onContinueRun: vi.fn(),
      });
      expect(queryByTestId(CHIP)).toBeNull();
    });

    it("does not render while the send RPC is in flight (sending)", () => {
      const { queryByTestId } = renderComposer({
        interrupted: true,
        sending: true,
        onContinueRun: vi.fn(),
      });
      expect(queryByTestId(CHIP)).toBeNull();
    });

    it("does not render without an onContinueRun handler", () => {
      const { queryByTestId } = renderComposer({ interrupted: true });
      expect(queryByTestId(CHIP)).toBeNull();
    });

    it("does not render when the thread is not interrupted", () => {
      const { queryByTestId } = renderComposer({
        interrupted: false,
        onContinueRun: vi.fn(),
      });
      expect(queryByTestId(CHIP)).toBeNull();
    });

    it("mounts the attachment strip when the chip is its only occupant", () => {
      // Default mode + no staged attachments would normally hide the whole
      // strip — the chip alone must be enough to mount it, and nothing
      // else (no mode pill, no attachment chips) should render inside.
      const { getByTestId } = renderComposer({
        interrupted: true,
        onContinueRun: vi.fn(),
        mode: "default",
        stagedAttachments: [],
      });
      const strip = getByTestId("composer-attachment-strip");
      const chip = getByTestId(CHIP);
      expect(strip).toContainElement(chip);
      // The chip is the strip's sole child.
      expect(strip.children).toHaveLength(1);
      expect(strip.firstElementChild).toBe(chip);
    });
  });
});
