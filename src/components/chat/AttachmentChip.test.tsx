/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";

import { AttachmentChip } from "./AttachmentChip";
import type { Attachment } from "@/stores/agent-chat-store";

afterEach(() => cleanup());

function makeAttachment(overrides: Partial<Attachment> = {}): Attachment {
  return {
    id: "att-1",
    kind: "file",
    ref: "src/components/chat/Composer.tsx",
    metadata: { label: "Composer.tsx", lineCount: 421 },
    ...overrides,
  };
}

describe("AttachmentChip", () => {
  describe("file kind (Stage 1 fully-tested path)", () => {
    it("renders the label and line count", () => {
      const { getByText } = render(
        <AttachmentChip attachment={makeAttachment()} onRemove={vi.fn()} />,
      );
      expect(getByText("Composer.tsx")).toBeInTheDocument();
      expect(getByText("421L")).toBeInTheDocument();
    });

    it("uses neutral color for files (no accent)", () => {
      const { getByRole } = render(
        <AttachmentChip attachment={makeAttachment()} onRemove={vi.fn()} />,
      );
      const chip = getByRole("status");
      expect(chip.className).toContain("bg-foreground/10");
      expect(chip.className).toContain("text-foreground");
    });

    it("renders a bordered card chip in its own tint (design D3/D10)", () => {
      // The redesign gives every staged ref the bordered-card look the
      // design shows on the green issue chip; the border colour tracks
      // the per-kind tint.
      const { getByRole } = render(
        <AttachmentChip attachment={makeAttachment()} onRemove={vi.fn()} />,
      );
      const chip = getByRole("status");
      expect(chip.className).toContain("border");
      expect(chip.className).toContain("border-border/60");
    });

    it("omits the line-count slot when lineCount is undefined", () => {
      const { queryByText } = render(
        <AttachmentChip
          attachment={makeAttachment({
            metadata: { label: "image.png" },
          })}
          onRemove={vi.fn()}
        />,
      );
      expect(queryByText(/L$/)).toBeNull();
    });
  });

  describe("loading state", () => {
    it("renders a spinner instead of the kind icon when isLoading is true", () => {
      const { getByTestId } = render(
        <AttachmentChip
          attachment={makeAttachment({
            metadata: { label: "Loading…", isLoading: true },
          })}
          onRemove={vi.fn()}
        />,
      );
      expect(getByTestId("attachment-chip-spinner")).toBeInTheDocument();
    });
  });

  describe("error state", () => {
    it("renders an error indicator when metadata.error is set", () => {
      const { getByLabelText } = render(
        <AttachmentChip
          attachment={makeAttachment({
            metadata: { label: "broken.ts", error: "fetch failed" },
          })}
          onRemove={vi.fn()}
        />,
      );
      expect(getByLabelText("error: fetch failed")).toBeInTheDocument();
    });
  });

  describe("removal", () => {
    it("calls onRemove with the attachment id when X is clicked", () => {
      const onRemove = vi.fn();
      const { getByLabelText } = render(
        <AttachmentChip attachment={makeAttachment()} onRemove={onRemove} />,
      );
      fireEvent.click(getByLabelText("Remove Composer.tsx"));
      expect(onRemove).toHaveBeenCalledWith("att-1");
    });

    it("stops propagation so the chip-body click handler does not also fire", () => {
      const onRemove = vi.fn();
      const onChipClick = vi.fn();
      const { getByLabelText, getByRole } = render(
        <div onClick={onChipClick}>
          <AttachmentChip attachment={makeAttachment()} onRemove={onRemove} />
        </div>,
      );
      const chip = getByRole("status");
      const x = getByLabelText("Remove Composer.tsx");
      // Click the chip body — outer handler fires.
      fireEvent.click(chip);
      expect(onChipClick).toHaveBeenCalledTimes(1);
      onChipClick.mockClear();
      // Click the X — outer handler should NOT fire.
      fireEvent.click(x);
      expect(onChipClick).not.toHaveBeenCalled();
      expect(onRemove).toHaveBeenCalledWith("att-1");
    });
  });

  describe("kind variants render distinct icons + colors", () => {
    it("folder uses the same neutral color as files but a different icon", () => {
      const { getByRole, getByLabelText } = render(
        <AttachmentChip
          attachment={makeAttachment({
            kind: "folder",
            metadata: { label: "src/components" },
          })}
          onRemove={vi.fn()}
        />,
      );
      expect(getByRole("status").className).toContain("bg-foreground/10");
      expect(getByLabelText(/folder attachment/)).toBeInTheDocument();
    });

    it("open issue uses the warning accent", () => {
      const { getByRole } = render(
        <AttachmentChip
          attachment={makeAttachment({
            kind: "issue",
            metadata: { label: "#123 · bug", state: "open" },
          })}
          onRemove={vi.fn()}
        />,
      );
      expect(getByRole("status").className).toContain("bg-warning/15");
    });

    it("closed issue falls back to muted neutral", () => {
      const { getByRole } = render(
        <AttachmentChip
          attachment={makeAttachment({
            kind: "issue",
            metadata: { label: "#123 · bug", state: "closed" },
          })}
          onRemove={vi.fn()}
        />,
      );
      expect(getByRole("status").className).toContain("text-muted-foreground");
    });

    it("open PR uses the primary accent", () => {
      const { getByRole } = render(
        <AttachmentChip
          attachment={makeAttachment({
            kind: "pr",
            metadata: { label: "#42 · feature", state: "open" },
          })}
          onRemove={vi.fn()}
        />,
      );
      expect(getByRole("status").className).toContain("bg-primary/15");
    });

    it("merged PR uses the merged-purple variant (chart-4 token)", () => {
      // Stage 5 — merged PRs were originally rendered with muted
      // neutral, but that conflated them with closed/draft. The new
      // contract: merged → `text-chart-4` purple to match the
      // `GitMerge` icon tint in PrPickerPanel + the chip strip.
      const { getByRole } = render(
        <AttachmentChip
          attachment={makeAttachment({
            kind: "pr",
            metadata: { label: "#42 · feature", state: "merged" },
          })}
          onRemove={vi.fn()}
        />,
      );
      const className = getByRole("status").className;
      expect(className).toContain("text-chart-4");
      expect(className).not.toContain("text-muted-foreground");
    });

    it("draft PR renders muted (in-progress, not yet up for review)", () => {
      const { getByRole } = render(
        <AttachmentChip
          attachment={makeAttachment({
            kind: "pr",
            metadata: { label: "#42 · WIP", state: "draft" },
          })}
          onRemove={vi.fn()}
        />,
      );
      expect(getByRole("status").className).toContain(
        "text-muted-foreground",
      );
    });

    it("closed PR renders muted neutral", () => {
      const { getByRole } = render(
        <AttachmentChip
          attachment={makeAttachment({
            kind: "pr",
            metadata: { label: "#42 · obsolete", state: "closed" },
          })}
          onRemove={vi.fn()}
        />,
      );
      expect(getByRole("status").className).toContain(
        "text-muted-foreground",
      );
    });

    it("image uses the accent variant", () => {
      const { getByRole } = render(
        <AttachmentChip
          attachment={makeAttachment({
            kind: "image",
            metadata: { label: "screenshot.png" },
          })}
          onRemove={vi.fn()}
        />,
      );
      expect(getByRole("status").className).toContain("bg-accent/15");
    });
  });

  it("exposes the kind via data-attachment-kind for parent styling", () => {
    const { container } = render(
      <AttachmentChip attachment={makeAttachment()} onRemove={vi.fn()} />,
    );
    expect(
      container.querySelector('[data-attachment-kind="file"]'),
    ).not.toBeNull();
  });

  // ──────────────── Stage 7 polish ────────────────

  describe("Stage 7 — truncation indicator", () => {
    it("renders 'first 50/N L' when metadata.isTruncated is true", () => {
      const { getByTestId, queryByText } = render(
        <AttachmentChip
          attachment={makeAttachment({
            metadata: {
              label: "huge.ts",
              lineCount: 3000,
              isTruncated: true,
            },
          })}
          onRemove={vi.fn()}
        />,
      );
      expect(getByTestId("attachment-chip-truncation").textContent).toBe(
        "first 50/3000L",
      );
      // The plain "3000L" line-count slot is suppressed when truncated
      // so we don't double-render the line count.
      expect(queryByText("3000L")).toBeNull();
    });

    it("renders the plain line-count slot when not truncated", () => {
      const { getByText, queryByTestId } = render(
        <AttachmentChip
          attachment={makeAttachment({
            metadata: { label: "small.ts", lineCount: 421 },
          })}
          onRemove={vi.fn()}
        />,
      );
      expect(getByText("421L")).toBeInTheDocument();
      expect(queryByTestId("attachment-chip-truncation")).toBeNull();
    });

    it("hides the truncation slot for non-file kinds even when isTruncated is set", () => {
      const { queryByTestId } = render(
        <AttachmentChip
          attachment={makeAttachment({
            kind: "folder",
            metadata: {
              label: "src",
              lineCount: 100,
              isTruncated: true,
            },
          })}
          onRemove={vi.fn()}
        />,
      );
      expect(queryByTestId("attachment-chip-truncation")).toBeNull();
    });
  });

  describe("Stage 7 — PR expand affordance", () => {
    function makePrAttachment(overrides: Partial<Attachment> = {}): Attachment {
      return {
        id: "pr-1",
        kind: "pr",
        ref: "!42",
        metadata: { label: "#42 dark mode", state: "open" },
        ...overrides,
      };
    }

    it("renders the expand button only when kind is pr and onToggleExpand is wired", () => {
      const { queryByTestId } = render(
        <AttachmentChip
          attachment={makePrAttachment()}
          onRemove={vi.fn()}
          onToggleExpand={vi.fn()}
        />,
      );
      expect(queryByTestId("attachment-chip-expand")).not.toBeNull();
    });

    it("omits the expand button when onToggleExpand is not provided", () => {
      const { queryByTestId } = render(
        <AttachmentChip
          attachment={makePrAttachment()}
          onRemove={vi.fn()}
        />,
      );
      expect(queryByTestId("attachment-chip-expand")).toBeNull();
    });

    it("omits the expand button on non-PR kinds", () => {
      const { queryByTestId } = render(
        <AttachmentChip
          attachment={makeAttachment({ kind: "file" })}
          onRemove={vi.fn()}
          onToggleExpand={vi.fn()}
        />,
      );
      expect(queryByTestId("attachment-chip-expand")).toBeNull();
    });

    it("calls onToggleExpand with the attachment id and stops propagation", () => {
      const onToggle = vi.fn();
      const onChipClick = vi.fn();
      const { getByTestId } = render(
        <div onClick={onChipClick}>
          <AttachmentChip
            attachment={makePrAttachment()}
            onRemove={vi.fn()}
            onToggleExpand={onToggle}
          />
        </div>,
      );
      fireEvent.click(getByTestId("attachment-chip-expand"));
      expect(onToggle).toHaveBeenCalledWith("pr-1");
      expect(onChipClick).not.toHaveBeenCalled();
    });

    it("flips aria-pressed and tooltip copy when expandFullDiff is true", () => {
      const { getByTestId } = render(
        <AttachmentChip
          attachment={makePrAttachment({
            metadata: {
              label: "#42 dark mode",
              state: "open",
              expandFullDiff: true,
            },
          })}
          onRemove={vi.fn()}
          onToggleExpand={vi.fn()}
        />,
      );
      const btn = getByTestId("attachment-chip-expand");
      expect(btn.getAttribute("aria-pressed")).toBe("true");
      expect(btn.getAttribute("aria-label")).toBe("Show filenames only");
    });

    it("hides the expand button while loading so the user can't double-toggle", () => {
      const { queryByTestId } = render(
        <AttachmentChip
          attachment={makePrAttachment({
            metadata: {
              label: "#42 dark mode",
              state: "open",
              isLoading: true,
            },
          })}
          onRemove={vi.fn()}
          onToggleExpand={vi.fn()}
        />,
      );
      expect(queryByTestId("attachment-chip-expand")).toBeNull();
    });
  });

  describe("Stage 7 — token-cost tooltip wrapper", () => {
    it("wraps the chip in a Radix tooltip trigger", () => {
      // Radix attaches `data-slot="tooltip-trigger"` to the trigger,
      // and the trigger forwards refs onto the chip via asChild. We
      // assert the trigger is present rather than opening the tooltip
      // (which requires a hover gesture).
      const { container } = render(
        <AttachmentChip attachment={makeAttachment()} onRemove={vi.fn()} />,
      );
      expect(
        container.querySelector('[data-slot="tooltip-trigger"]'),
      ).not.toBeNull();
    });
  });
});
