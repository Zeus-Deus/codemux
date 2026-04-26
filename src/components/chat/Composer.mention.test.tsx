/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  waitFor,
  type RenderResult,
} from "@testing-library/react";
import { useState, type ComponentProps } from "react";

import { TooltipProvider } from "@/components/ui/tooltip";
import type { FileMatch } from "@/tauri/types";

vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return {
    ...actual,
    listProjectFiles: vi.fn(),
    // Skills loader is unrelated but the Composer pulls it from the
    // store on slash-popup open; stub it so we don't trip the real
    // invoke from inside the mention tests.
    listSkills: vi.fn().mockResolvedValue([]),
  };
});

import { Composer } from "./Composer";
import { listProjectFiles } from "@/tauri/commands";

type ComposerProps = ComponentProps<typeof Composer>;

const listProjectFilesMock =
  listProjectFiles as unknown as ReturnType<typeof vi.fn>;

function makeMatch(overrides: Partial<FileMatch> = {}): FileMatch {
  return {
    path: "src/components/chat/Composer.tsx",
    absolute_path: "/repo/src/components/chat/Composer.tsx",
    score: 200,
    ...overrides,
  };
}

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

function renderComposer(
  props: Partial<ComposerProps> = {},
): RenderResult {
  return render(
    <TooltipProvider>
      <Composer {...baseProps()} {...props} />
    </TooltipProvider>,
  );
}

/** Controlled wrapper that mirrors the real `AgentChatPane` behavior:
 *  the textarea writes to a parent state that flows back as the
 *  `draft` prop. Without this, the Composer's strip-on-pick reads a
 *  stale prop and asserts trip on empty strings. */
function ControlledComposer(props: Partial<ComposerProps> = {}) {
  const [draft, setDraft] = useState(props.draft ?? "");
  const baselineProps = baseProps();
  return (
    <Composer
      {...baselineProps}
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

beforeEach(() => {
  listProjectFilesMock.mockReset();
  listProjectFilesMock.mockResolvedValue([]);
});

afterEach(() => cleanup());

function typeIntoTextarea(value: string, cursor?: number): HTMLTextAreaElement {
  const textarea = document.querySelector(
    'textarea',
  ) as HTMLTextAreaElement;
  fireEvent.change(textarea, {
    target: {
      value,
      selectionStart: cursor ?? value.length,
      selectionEnd: cursor ?? value.length,
    },
  });
  return textarea;
}

describe("Composer @ mention popup (Step 8 Stage 2)", () => {
  it("fetches files via listProjectFiles when the user types @<query>", async () => {
    listProjectFilesMock.mockResolvedValue([makeMatch()]);
    const onDraftChange = vi.fn();
    renderComposer({ draft: "", onDraftChange });
    typeIntoTextarea("@composer");
    await waitFor(() => {
      expect(listProjectFilesMock).toHaveBeenCalled();
    });
    const [cwdArg, queryArg, limitArg] = listProjectFilesMock.mock.calls[0]!;
    expect(cwdArg).toBe("/repo");
    expect(queryArg).toBe("composer");
    expect(typeof limitArg).toBe("number");
  });

  it("requests an alphabetical listing (null query) on bare @", async () => {
    listProjectFilesMock.mockResolvedValue([]);
    renderComposer();
    typeIntoTextarea("@");
    await waitFor(() => {
      expect(listProjectFilesMock).toHaveBeenCalled();
    });
    const [, queryArg] = listProjectFilesMock.mock.calls[0]!;
    expect(queryArg).toBeNull();
  });

  it("renders fetched matches as picker rows", async () => {
    listProjectFilesMock.mockResolvedValue([
      makeMatch({ path: "src/components/chat/Composer.tsx" }),
      makeMatch({
        path: "src/components/chat/ComposerFooter.tsx",
        absolute_path: "/repo/src/components/chat/ComposerFooter.tsx",
      }),
    ]);
    const { findByText } = renderComposer();
    typeIntoTextarea("@comp");
    expect(
      await findByText("src/components/chat/Composer.tsx"),
    ).toBeInTheDocument();
    expect(
      await findByText("src/components/chat/ComposerFooter.tsx"),
    ).toBeInTheDocument();
  });

  it("calls onAttachFile and inserts an @<basename> token in the textarea", async () => {
    // Stage 2.1 — inline-chip model: picking a file replaces the
    // typed `@<query>` with `@<basename> ` so the token stays in the
    // textarea and the mirror renders it as an inline chip.
    const match = makeMatch();
    listProjectFilesMock.mockResolvedValue([match]);
    const onAttachFile = vi.fn();
    const onDraftChange = vi.fn();
    const { findByText } = renderControlled({
      onAttachFile,
      onDraftChange,
    });
    typeIntoTextarea("@composer");
    const row = await findByText("src/components/chat/Composer.tsx");
    fireEvent.click(row);
    expect(onAttachFile).toHaveBeenCalledWith(match);
    const calls = onDraftChange.mock.calls;
    const lastDraftCall = calls[calls.length - 1];
    expect(lastDraftCall?.[0]).toBe("@Composer.tsx ");
  });

  it("replaces the typed @<query> with the @<basename> token, preserving prose", async () => {
    // Stage 2.1 — typing prose then `@<query>` and picking inserts
    // the basename back where the query was: "hello @utils" → pick
    // utils.ts → "hello @utils.ts " (trailing space lets the user
    // keep typing without a manual keystroke).
    const match = makeMatch({
      path: "src/lib/utils.ts",
      absolute_path: "/repo/src/lib/utils.ts",
    });
    listProjectFilesMock.mockResolvedValue([match]);
    const onAttachFile = vi.fn();
    const onDraftChange = vi.fn();
    const { findByText } = renderControlled({
      onAttachFile,
      onDraftChange,
    });
    typeIntoTextarea("hello @utils");
    const row = await findByText("src/lib/utils.ts");
    fireEvent.click(row);
    expect(onAttachFile).toHaveBeenCalledWith(match);
    const calls = onDraftChange.mock.calls;
    const finalText = calls[calls.length - 1]?.[0];
    expect(finalText).toBe("hello @utils.ts ");
  });

  it("Esc closes the mention popup without picking", async () => {
    listProjectFilesMock.mockResolvedValue([makeMatch()]);
    const onAttachFile = vi.fn();
    const { findByText, queryByText } = renderComposer({ onAttachFile });
    typeIntoTextarea("@comp");
    await findByText("src/components/chat/Composer.tsx");
    const textarea = document.querySelector(
      'textarea',
    ) as HTMLTextAreaElement;
    fireEvent.keyDown(textarea, { key: "Escape" });
    await waitFor(() => {
      expect(
        queryByText("src/components/chat/Composer.tsx"),
      ).toBeNull();
    });
    expect(onAttachFile).not.toHaveBeenCalled();
  });

  it("shows the project-required footer hint when cwd is null", async () => {
    const { findByText } = renderComposer({ cwd: null });
    typeIntoTextarea("@");
    expect(
      await findByText("Open this chat in a project to attach files."),
    ).toBeInTheDocument();
    // No fetch should fire when there's nothing to scan.
    expect(listProjectFilesMock).not.toHaveBeenCalled();
  });

  it("does not fire the popup on a slash trigger (slash and mention are exclusive)", async () => {
    renderComposer();
    typeIntoTextarea("/plan");
    // The slash popup uses a different fetch path; mention's listProjectFiles
    // mock should not have been touched.
    expect(listProjectFilesMock).not.toHaveBeenCalled();
  });
});
