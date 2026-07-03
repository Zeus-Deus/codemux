/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  type RenderResult,
} from "@testing-library/react";
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

function renderControlled(
  props: Partial<ComposerProps> = {},
): RenderResult {
  return render(
    <TooltipProvider>
      <ControlledComposer {...props} />
    </TooltipProvider>,
  );
}

afterEach(() => cleanup());

// JSDOM's File doesn't implement arrayBuffer in older environments.
// Force a stable polyfill so the Composer's await file.arrayBuffer()
// path is exercised the same way it would be in the browser.
beforeEach(() => {
  if (
    typeof File !== "undefined" &&
    !File.prototype.arrayBuffer
  ) {
    File.prototype.arrayBuffer = function () {
      return Promise.resolve(new ArrayBuffer(0));
    };
  }
});

function pngFile(name = "screenshot.png"): File {
  return new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], name, {
    type: "image/png",
  });
}

describe("Composer image attach (Step 8 Stage 6)", () => {
  describe("paste handler", () => {
    it("calls onAttachImage when an image file is on the clipboard", () => {
      const onAttachImage = vi.fn();
      const { container } = renderControlled({ onAttachImage });
      const textarea = container.querySelector(
        "textarea",
      ) as HTMLTextAreaElement;

      const file = pngFile();
      const items: DataTransferItem[] = [
        {
          kind: "file",
          type: "image/png",
          getAsFile: () => file,
          getAsString: () => {},
          webkitGetAsEntry: () => null,
        } as unknown as DataTransferItem,
      ];

      fireEvent.paste(textarea, {
        clipboardData: {
          items,
          files: [file],
          types: ["Files"],
          getData: () => "",
        },
      });

      expect(onAttachImage).toHaveBeenCalledTimes(1);
      expect(onAttachImage).toHaveBeenCalledWith(file);
    });

    it("forwards every image when multiple are pasted at once", async () => {
      const onAttachImage = vi.fn();
      const { container } = renderControlled({ onAttachImage });
      const textarea = container.querySelector(
        "textarea",
      ) as HTMLTextAreaElement;

      const a = pngFile("a.png");
      const b = pngFile("b.png");
      const items: DataTransferItem[] = [
        {
          kind: "file",
          type: "image/png",
          getAsFile: () => a,
        } as unknown as DataTransferItem,
        {
          kind: "file",
          type: "image/png",
          getAsFile: () => b,
        } as unknown as DataTransferItem,
      ];

      fireEvent.paste(textarea, {
        clipboardData: {
          items,
          files: [a, b],
          types: ["Files"],
          getData: () => "",
        },
      });

      // The handler awaits each onAttachImage call serially; flush
      // pending microtasks so all iterations land before the assert.
      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(onAttachImage).toHaveBeenCalledTimes(2);
      expect(onAttachImage.mock.calls[0]?.[0]).toBe(a);
      expect(onAttachImage.mock.calls[1]?.[0]).toBe(b);
    });

    it("does NOT intercept plain-text paste", () => {
      // Regression check: pasting "hello world" into the composer
      // must still type into the textarea. We assert this by
      // verifying onAttachImage is never called for a non-image
      // clipboard payload.
      const onAttachImage = vi.fn();
      const onDraftChange = vi.fn();
      const { container } = renderControlled({
        onAttachImage,
        onDraftChange,
      });
      const textarea = container.querySelector(
        "textarea",
      ) as HTMLTextAreaElement;

      fireEvent.paste(textarea, {
        clipboardData: {
          items: [
            {
              kind: "string",
              type: "text/plain",
              getAsFile: () => null,
            } as unknown as DataTransferItem,
          ],
          files: [],
          types: ["text/plain"],
          getData: () => "hello world",
        },
      });

      expect(onAttachImage).not.toHaveBeenCalled();
    });

    it("forwards SVGs verbatim — Composer doesn't validate, the parent does", () => {
      // Locked decision: type validation lives in the parent's
      // handleAttachImage (which surfaces the rejection toast).
      // Composer just hands the File over so the validation is
      // testable in isolation.
      const onAttachImage = vi.fn();
      const { container } = renderControlled({ onAttachImage });
      const textarea = container.querySelector(
        "textarea",
      ) as HTMLTextAreaElement;

      const svg = new File(["<svg/>"], "diagram.svg", {
        type: "image/svg+xml",
      });

      fireEvent.paste(textarea, {
        clipboardData: {
          items: [
            {
              kind: "file",
              type: "image/svg+xml",
              getAsFile: () => svg,
            } as unknown as DataTransferItem,
          ],
          files: [svg],
          types: ["Files"],
          getData: () => "",
        },
      });

      expect(onAttachImage).toHaveBeenCalledWith(svg);
    });
  });

  describe("drop handler", () => {
    it("calls onAttachImage when image files are dropped on the composer", () => {
      const onAttachImage = vi.fn();
      const { container } = renderControlled({ onAttachImage });
      // The handler is attached to the rounded composer wrapper —
      // grab it via the textarea's parent chain since the wrapper
      // doesn't carry its own test id.
      const textarea = container.querySelector(
        "textarea",
      ) as HTMLTextAreaElement;
      // Walk up to the outermost composer card. Three levels: the
      // textarea sits inside a `relative` wrapper which sits inside
      // the rounded card.
      const wrapper = textarea.closest('[data-testid="composer-wrapper"]')!;

      const file = pngFile();
      fireEvent.drop(wrapper, {
        dataTransfer: { files: [file], types: ["Files"] },
      });

      expect(onAttachImage).toHaveBeenCalledWith(file);
    });

    it("dragOver with Files calls preventDefault so the drop is accepted", () => {
      const onAttachImage = vi.fn();
      const { container } = renderControlled({ onAttachImage });
      const textarea = container.querySelector(
        "textarea",
      ) as HTMLTextAreaElement;
      const wrapper = textarea.closest('[data-testid="composer-wrapper"]')!;

      const event = new Event("dragover", { bubbles: true, cancelable: true });
      // Pollute event with a synthetic dataTransfer so the handler
      // can read .types.
      Object.defineProperty(event, "dataTransfer", {
        value: { types: ["Files"] },
      });
      wrapper.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(true);
    });

    it("ignores non-image files on drop", () => {
      const onAttachImage = vi.fn();
      const { container } = renderControlled({ onAttachImage });
      const textarea = container.querySelector(
        "textarea",
      ) as HTMLTextAreaElement;
      const wrapper = textarea.closest('[data-testid="composer-wrapper"]')!;

      const txtFile = new File(["hi"], "notes.txt", {
        type: "text/plain",
      });
      fireEvent.drop(wrapper, {
        dataTransfer: { files: [txtFile], types: ["Files"] },
      });
      expect(onAttachImage).not.toHaveBeenCalled();
    });
  });

  describe("+ → Image… picker entry", () => {
    it("renders enabled when modelSupportsImages is true", () => {
      const { getByTestId } = renderControlled({
        modelSupportsImages: true,
      });
      fireEvent.click(getByTestId("composer-attach-button"));
      const row = getByTestId("slash-item-attach:image");
      expect(row.getAttribute("data-disabled")).not.toBe("true");
    });

    it("renders disabled with a hint when modelSupportsImages is false", () => {
      const { getByTestId, getByText } = renderControlled({
        modelSupportsImages: false,
      });
      fireEvent.click(getByTestId("composer-attach-button"));
      const row = getByTestId("slash-item-attach:image");
      expect(row.getAttribute("data-disabled")).toBe("true");
      expect(
        getByText("Current model doesn't support images"),
      ).toBeInTheDocument();
    });

    it("clicking the enabled entry triggers the hidden file input", () => {
      // We can't open the OS file dialog under JSDOM; assert
      // that the input's `.click()` was invoked instead. That's
      // the contract the rest of the flow depends on.
      const onAttachImage = vi.fn();
      const { getByTestId } = renderControlled({
        modelSupportsImages: true,
        onAttachImage,
      });
      fireEvent.click(getByTestId("composer-attach-button"));
      const fileInput = getByTestId(
        "composer-image-file-input",
      ) as HTMLInputElement;
      const clickSpy = vi.spyOn(fileInput, "click");
      fireEvent.click(getByTestId("slash-item-attach:image"));
      // The actual click is dispatched in a requestAnimationFrame
      // so the popup unmount completes first. Flush rAF.
      return new Promise<void>((resolve) => {
        requestAnimationFrame(() => {
          expect(clickSpy).toHaveBeenCalled();
          resolve();
        });
      });
    });

    it("the hidden file input forwards each picked File to onAttachImage", async () => {
      const onAttachImage = vi.fn();
      const { getByTestId } = renderControlled({ onAttachImage });
      const input = getByTestId(
        "composer-image-file-input",
      ) as HTMLInputElement;
      const a = pngFile("one.png");
      const b = pngFile("two.png");
      // JSDOM doesn't allow direct assignment to .files; mock it
      // with defineProperty so onChange sees the values.
      Object.defineProperty(input, "files", { value: [a, b] });
      fireEvent.change(input);
      // The handler awaits each call; flush microtasks so both
      // iterations land before the assert.
      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(onAttachImage).toHaveBeenCalledTimes(2);
      expect(onAttachImage.mock.calls[0]?.[0]).toBe(a);
      expect(onAttachImage.mock.calls[1]?.[0]).toBe(b);
    });
  });

  describe("5MB soft warning", () => {
    let nextId = 0;
    function imgAttachment(bytes: number) {
      const id = `img-${++nextId}`;
      return {
        id,
        kind: "image" as const,
        ref: `image:${id}`,
        metadata: { label: "x.png", bytes },
        resolvedImage: {
          mime: "image/png",
          // Allocating a real 5MB Uint8Array makes the test slow on
          // CI; use a length-only stub. The total-bytes computation
          // reads `bytes.length` only.
          bytes: { length: bytes } as unknown as Uint8Array,
        },
      };
    }

    it("hides the warning below the 5MB threshold", () => {
      const { queryByTestId } = renderControlled({
        stagedAttachments: [imgAttachment(2 * 1024 * 1024)],
      });
      expect(queryByTestId("composer-image-size-warning")).toBeNull();
    });

    it("shows the warning when total image bytes exceeds 5MB", () => {
      const { getByTestId } = renderControlled({
        stagedAttachments: [
          imgAttachment(3 * 1024 * 1024),
          imgAttachment(3 * 1024 * 1024),
        ],
      });
      const warning = getByTestId("composer-image-size-warning");
      expect(warning.textContent).toContain("Total image size");
      expect(warning.textContent).toContain("MB");
    });
  });
});
