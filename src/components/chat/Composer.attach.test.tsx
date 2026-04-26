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
import type { FolderMatch, FileMatch } from "@/tauri/types";

vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return {
    ...actual,
    listProjectFiles: vi.fn().mockResolvedValue([]),
    listProjectFolders: vi.fn().mockResolvedValue([]),
    listSkills: vi.fn().mockResolvedValue([]),
  };
});

import { Composer } from "./Composer";
import { listProjectFiles, listProjectFolders } from "@/tauri/commands";

type ComposerProps = ComponentProps<typeof Composer>;

const listProjectFilesMock = listProjectFiles as unknown as ReturnType<typeof vi.fn>;
const listProjectFoldersMock = listProjectFolders as unknown as ReturnType<typeof vi.fn>;

function makeFile(overrides: Partial<FileMatch> = {}): FileMatch {
  return {
    path: "src/components/chat/Composer.tsx",
    absolute_path: "/repo/src/components/chat/Composer.tsx",
    score: 0,
    ...overrides,
  };
}

function makeFolder(overrides: Partial<FolderMatch> = {}): FolderMatch {
  return {
    path: "src/components/chat",
    absolute_path: "/repo/src/components/chat",
    score: 0,
    item_count: 5,
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

beforeEach(() => {
  listProjectFilesMock.mockReset();
  listProjectFoldersMock.mockReset();
  listProjectFilesMock.mockResolvedValue([]);
  listProjectFoldersMock.mockResolvedValue([]);
});

afterEach(() => cleanup());

describe("Composer + button + attach popup (Step 8 Stage 3)", () => {
  it("the + button is always visible in the footer (locked decision: persistent)", () => {
    const { getByTestId } = renderControlled();
    expect(getByTestId("composer-attach-button")).toBeInTheDocument();
  });

  it("clicking the + button opens the popup with MODES + ATTACH groups", () => {
    const { getByTestId, getByText, queryByText } = renderControlled();
    fireEvent.click(getByTestId("composer-attach-button"));
    // MODES at the top — primary affordance now that the `+ Mode`
    // dropdown is gone.
    expect(getByText("Plan")).toBeInTheDocument();
    expect(getByText("Debug")).toBeInTheDocument();
    expect(getByText("Ask")).toBeInTheDocument();
    // ATTACH below.
    expect(getByText("File…")).toBeInTheDocument();
    expect(getByText("Folder…")).toBeInTheDocument();
    expect(getByText("GitHub Issue…")).toBeInTheDocument();
    // NAVIGATION group dropped — `/` and `@` keyboard paths stay
    // self-evident; the MODES section makes them discoverable.
    expect(queryByText("Slash commands")).toBeNull();
    expect(queryByText("Mention")).toBeNull();
  });

  it("picking Plan from the + popup activates Plan mode + closes the popup", () => {
    const onModeActivate = vi.fn();
    const { getByTestId, queryByText } = renderControlled({
      onModeActivate,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-mode:plan"));
    expect(onModeActivate).toHaveBeenCalledWith("plan");
    expect(queryByText("Plan")).toBeNull();
  });

  it("picking Debug from the + popup activates Debug mode", () => {
    const onModeActivate = vi.fn();
    const { getByTestId } = renderControlled({ onModeActivate });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-mode:debug"));
    expect(onModeActivate).toHaveBeenCalledWith("debug");
  });

  it("picking Ask from the + popup activates Ask mode", () => {
    const onModeActivate = vi.fn();
    const { getByTestId } = renderControlled({ onModeActivate });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-mode:ask"));
    expect(onModeActivate).toHaveBeenCalledWith("ask");
  });

  it("the currently-active mode renders disabled in the + popup (no double-activate)", () => {
    const onModeActivate = vi.fn();
    const { getByTestId } = renderControlled({
      mode: "plan",
      onModeActivate,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    const plan = getByTestId("slash-item-mode:plan");
    expect(plan.getAttribute("data-disabled")).toBe("true");
    fireEvent.click(plan);
    expect(onModeActivate).not.toHaveBeenCalled();
  });

  it("disabled coming-soon entries render with the disabled attribute", () => {
    const { getByTestId } = renderControlled();
    fireEvent.click(getByTestId("composer-attach-button"));
    const issueRow = getByTestId("slash-item-attach:issue");
    const prRow = getByTestId("slash-item-attach:pr");
    const imageRow = getByTestId("slash-item-attach:image");
    expect(issueRow.getAttribute("data-disabled")).toBe("true");
    expect(prRow.getAttribute("data-disabled")).toBe("true");
    expect(imageRow.getAttribute("data-disabled")).toBe("true");
    expect(issueRow.className).toContain("opacity-50");
  });

  it("clicking a disabled entry is a no-op (popup stays on main)", async () => {
    const { getByTestId, getByText } = renderControlled();
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:issue"));
    // Still on main — File… still visible.
    expect(getByText("File…")).toBeInTheDocument();
  });

  it("picking File… pivots to the file submode and fetches files", async () => {
    listProjectFilesMock.mockResolvedValue([makeFile()]);
    const { getByTestId, findByText } = renderControlled({
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:file"));
    expect(
      await findByText("src/components/chat/Composer.tsx"),
    ).toBeInTheDocument();
    expect(listProjectFilesMock).toHaveBeenCalledWith("/repo", null, 30);
  });

  it("picking Folder… pivots to the folder submode and fetches folders", async () => {
    listProjectFoldersMock.mockResolvedValue([makeFolder()]);
    const { getByTestId, findByText } = renderControlled({
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:folder"));
    expect(
      await findByText("src/components/chat"),
    ).toBeInTheDocument();
    expect(listProjectFoldersMock).toHaveBeenCalledWith("/repo", null, 30);
  });

  it("picking a folder inserts the inline @<basename> token + calls onAttachFolder", async () => {
    listProjectFoldersMock.mockResolvedValue([makeFolder()]);
    const onAttachFolder = vi.fn();
    const onDraftChange = vi.fn();
    const { getByTestId, findByText } = renderControlled({
      onAttachFolder,
      onDraftChange,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:folder"));
    const row = await findByText("src/components/chat");
    fireEvent.click(row);
    expect(onAttachFolder).toHaveBeenCalledWith(makeFolder());
    const calls = onDraftChange.mock.calls;
    const finalText = calls[calls.length - 1]?.[0];
    expect(finalText).toBe("@chat ");
  });

  it("picking a file from the + → File… view inserts the inline @<basename> token", async () => {
    listProjectFilesMock.mockResolvedValue([makeFile()]);
    const onAttachFile = vi.fn();
    const onDraftChange = vi.fn();
    const { getByTestId, findByText } = renderControlled({
      onAttachFile,
      onDraftChange,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:file"));
    const row = await findByText("src/components/chat/Composer.tsx");
    fireEvent.click(row);
    expect(onAttachFile).toHaveBeenCalledWith(makeFile());
    const calls = onDraftChange.mock.calls;
    const finalText = calls[calls.length - 1]?.[0];
    expect(finalText).toBe("@Composer.tsx ");
  });

  it("Esc inside a submode walks back to main", async () => {
    listProjectFoldersMock.mockResolvedValue([makeFolder()]);
    const { getByTestId, findByText, getByText, queryByText } =
      renderControlled();
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:folder"));
    await findByText("src/components/chat");
    const textarea = document.querySelector(
      'textarea',
    ) as HTMLTextAreaElement;
    fireEvent.keyDown(textarea, { key: "Escape" });
    // Back on main: File… visible, folder rows gone.
    expect(getByText("File…")).toBeInTheDocument();
    expect(queryByText("src/components/chat")).toBeNull();
  });

  it("Esc on main closes the popup entirely", () => {
    const { getByTestId, queryByText } = renderControlled({
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    const textarea = document.querySelector(
      'textarea',
    ) as HTMLTextAreaElement;
    fireEvent.keyDown(textarea, { key: "Escape" });
    expect(queryByText("File…")).toBeNull();
  });

  it("clicking the button while open toggles the popup closed", () => {
    const { getByTestId, queryByText } = renderControlled({
    });
    const btn = getByTestId("composer-attach-button");
    fireEvent.click(btn);
    expect(queryByText("File…")).not.toBeNull();
    fireEvent.click(btn);
    expect(queryByText("File…")).toBeNull();
  });

  it("opening the attach popup closes any open mention popup", async () => {
    listProjectFilesMock.mockResolvedValue([makeFile()]);
    const { getByTestId, findByText, queryByText } = renderControlled({
    });
    // Open mention popup by typing @
    const textarea = document.querySelector(
      'textarea',
    ) as HTMLTextAreaElement;
    fireEvent.change(textarea, {
      target: {
        value: "@composer",
        selectionStart: 9,
        selectionEnd: 9,
      },
    });
    await findByText("src/components/chat/Composer.tsx");
    // Now click the + button — mention popup closes, attach popup opens
    fireEvent.click(getByTestId("composer-attach-button"));
    // The mention row should no longer be visible (the file submode
    // hasn't been picked yet so no row should show).
    expect(queryByText("src/components/chat/Composer.tsx")).toBeNull();
    expect(queryByText("File…")).toBeInTheDocument();
  });

  it("renders the loading footer note while folders are being fetched", async () => {
    let resolve!: (m: FolderMatch[]) => void;
    listProjectFoldersMock.mockReturnValue(
      new Promise<FolderMatch[]>((r) => {
        resolve = r;
      }),
    );
    const { getByTestId } = renderControlled();
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:folder"));
    await waitFor(() => {
      const footer = getByTestId("slash-popup-footer");
      expect(footer.textContent).toContain("Loading folders");
    });
    resolve([]);
  });

  it("shows the cwd-null hint when no project is anchored and a submode is opened", () => {
    const { getByTestId } = renderControlled({
      cwd: null,
    });
    fireEvent.click(getByTestId("composer-attach-button"));
    fireEvent.click(getByTestId("slash-item-attach:file"));
    const footer = getByTestId("slash-popup-footer");
    expect(footer.textContent).toContain("project");
  });
});
