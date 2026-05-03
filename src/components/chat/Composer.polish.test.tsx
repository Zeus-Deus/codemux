/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";
import { useState, type ComponentProps } from "react";

import { TooltipProvider } from "@/components/ui/tooltip";

vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return {
    ...actual,
    listProjectFiles: vi.fn().mockResolvedValue([]),
    listProjectFolders: vi.fn().mockResolvedValue([]),
    listGithubIssuesByPath: vi.fn().mockResolvedValue([]),
    getGithubIssueByPath: vi.fn(),
    listSkills: vi.fn().mockResolvedValue([]),
  };
});

import { Composer } from "./Composer";
import type { Attachment } from "@/stores/agent-chat-store";

type ComposerProps = ComponentProps<typeof Composer>;

function baseProps(): ComposerProps {
  return {
    draft: "",
    cwd: "/repo",
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
    onProviderChange: vi.fn(),
    onModelChange: vi.fn(),
    onPermissionModeChange: vi.fn(),
    onEffortChange: vi.fn(),
    onContextWindowChange: vi.fn(),
    onModeActivate: vi.fn(),
    onModeRemove: vi.fn(),
  };
}

function ControlledComposer(props: Partial<ComposerProps> = {}) {
  const [draft, setDraft] = useState(props.draft ?? "");
  const baseline = baseProps();
  return (
    <Composer
      {...baseline}
      {...props}
      draft={draft}
      onDraftChange={(next) => {
        setDraft(next);
        props.onDraftChange?.(next);
      }}
    />
  );
}

function renderControlled(props: Partial<ComposerProps> = {}) {
  return render(
    <TooltipProvider>
      <ControlledComposer {...props} />
    </TooltipProvider>,
  );
}

afterEach(() => cleanup());

let attachmentId = 0;
function fileAttachment(label = "Composer.tsx", overrides: Partial<Attachment> = {}): Attachment {
  return {
    id: `file-${++attachmentId}`,
    kind: "file",
    ref: `/repo/${label}`,
    metadata: { label, lineCount: 100 },
    ...overrides,
  };
}

function prAttachment(num = 42, overrides: Partial<Attachment> = {}): Attachment {
  return {
    id: `pr-${++attachmentId}`,
    kind: "pr",
    ref: `!${num}`,
    metadata: { label: `#${num} feat`, state: "open" },
    ...overrides,
  };
}

describe("Composer Stage 7 polish", () => {
  describe("drag-drop visual feedback", () => {
    it("does not show the dragging state initially", () => {
      const { getByTestId } = renderControlled();
      const wrapper = getByTestId("composer-wrapper");
      expect(wrapper.getAttribute("data-dragging")).toBeNull();
      expect(wrapper.className).not.toContain("ring-primary");
    });

    it("flips data-dragging on dragenter with Files payload", () => {
      const { getByTestId } = renderControlled();
      const wrapper = getByTestId("composer-wrapper");
      fireEvent.dragEnter(wrapper, {
        dataTransfer: { types: ["Files"], files: [] },
      });
      expect(wrapper.getAttribute("data-dragging")).toBe("true");
      expect(wrapper.className).toContain("ring-primary");
    });

    it("clears the dragging state on a matching dragleave", () => {
      const { getByTestId } = renderControlled();
      const wrapper = getByTestId("composer-wrapper");
      fireEvent.dragEnter(wrapper, {
        dataTransfer: { types: ["Files"] },
      });
      fireEvent.dragLeave(wrapper, {
        dataTransfer: { types: ["Files"] },
      });
      expect(wrapper.getAttribute("data-dragging")).toBeNull();
    });

    it("ignores non-Files drags so text-selection drag doesn't trigger", () => {
      const { getByTestId } = renderControlled();
      const wrapper = getByTestId("composer-wrapper");
      fireEvent.dragEnter(wrapper, {
        dataTransfer: { types: ["text/plain"] },
      });
      expect(wrapper.getAttribute("data-dragging")).toBeNull();
    });

    it("clears the dragging state on drop even when no images are present", () => {
      const { getByTestId } = renderControlled();
      const wrapper = getByTestId("composer-wrapper");
      fireEvent.dragEnter(wrapper, {
        dataTransfer: { types: ["Files"] },
      });
      fireEvent.drop(wrapper, {
        dataTransfer: { types: ["Files"], files: [] },
      });
      expect(wrapper.getAttribute("data-dragging")).toBeNull();
    });
  });

  describe("attachment count warnings", () => {
    function manyFiles(count: number): Attachment[] {
      return Array.from({ length: count }, (_, i) =>
        fileAttachment(`f${i}.ts`),
      );
    }

    it("hides both warnings below the soft limit", () => {
      const { queryByTestId } = renderControlled({
        stagedAttachments: manyFiles(5),
      });
      expect(
        queryByTestId("composer-attachment-count-warning"),
      ).toBeNull();
      expect(
        queryByTestId("composer-attachment-count-hardcap"),
      ).toBeNull();
    });

    it("shows the soft warning at 10 attachments", () => {
      const { getByTestId, queryByTestId } = renderControlled({
        stagedAttachments: manyFiles(10),
      });
      const warn = getByTestId("composer-attachment-count-warning");
      expect(warn.textContent).toContain("10 attachments");
      expect(
        queryByTestId("composer-attachment-count-hardcap"),
      ).toBeNull();
    });

    it("swaps to the hard-cap copy at 20 attachments", () => {
      const { getByTestId, queryByTestId } = renderControlled({
        stagedAttachments: manyFiles(20),
      });
      expect(
        queryByTestId("composer-attachment-count-warning"),
      ).toBeNull();
      const cap = getByTestId("composer-attachment-count-hardcap");
      expect(cap.textContent).toContain("limit reached");
    });
  });

  describe("PR chip strip + expand wiring", () => {
    it("renders PR chips in the strip alongside images", () => {
      const { getByTestId } = renderControlled({
        stagedAttachments: [prAttachment(42)],
      });
      const strip = getByTestId("composer-attachment-strip");
      expect(strip.querySelector('[data-attachment-kind="pr"]')).not.toBeNull();
    });

    it("PR expand button forwards to onToggleExpandPr with the attachment id", () => {
      const onToggleExpandPr = vi.fn();
      const { getByTestId } = renderControlled({
        stagedAttachments: [prAttachment(42, { id: "pr-fixed" })],
        onToggleExpandPr,
      });
      fireEvent.click(getByTestId("attachment-chip-expand"));
      expect(onToggleExpandPr).toHaveBeenCalledWith("pr-fixed");
    });

    it("PR expand button is hidden when onToggleExpandPr is not wired", () => {
      const { queryByTestId } = renderControlled({
        stagedAttachments: [prAttachment(42)],
      });
      expect(queryByTestId("attachment-chip-expand")).toBeNull();
    });
  });
});
