/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, act } from "@testing-library/react";

// Persisted UI state, keyed exactly as the store writes it. Tests seed this
// before render to simulate an install that already has a saved avatar.
let persisted: Record<string, string> = {};

vi.mock("@/tauri/commands", () => ({
  dbGetUiState: vi.fn((key: string) => Promise.resolve(persisted[key] ?? null)),
  dbSetUiState: vi.fn((key: string, value: string) => {
    persisted[key] = value;
    return Promise.resolve();
  }),
}));

// Radix's context menu portals through the body and is pointer-driven, which
// jsdom handles poorly. Render the primitives inline — the items and their
// handlers are what this suite is about. (Same approach as
// sidebar-workspace-row.test.tsx.)
vi.mock("@/components/ui/context-menu", () => {
  const passthrough = ({ children }: { children?: React.ReactNode }) => (
    <>{children}</>
  );
  return {
    ContextMenu: passthrough,
    ContextMenuTrigger: passthrough,
    ContextMenuContent: ({ children }: { children?: React.ReactNode }) => (
      <div role="menu">{children}</div>
    ),
    ContextMenuItem: ({
      children,
      onClick,
    }: {
      children?: React.ReactNode;
      onClick?: () => void;
    }) => (
      <button type="button" role="menuitem" onClick={onClick}>
        {children}
      </button>
    ),
    ContextMenuLabel: ({ children }: { children?: React.ReactNode }) => (
      <div>{children}</div>
    ),
    ContextMenuSeparator: () => <hr />,
    ContextMenuSub: passthrough,
    ContextMenuSubTrigger: ({ children }: { children?: React.ReactNode }) => (
      <div>{children}</div>
    ),
    ContextMenuSubContent: passthrough,
  };
});

// Late imports so the mocks above apply.
import { ProjectAppearanceMenu } from "./project-appearance-menu";
import { useProjectAppearance } from "./use-project-appearance";
import {
  __resetProjectAppearanceStoreForTests,
  useProjectAppearanceStore,
} from "@/stores/project-appearance-store";
import { dbGetUiState, dbSetUiState } from "@/tauri/commands";

const PATH = "/home/u/projects/codemux";

/** A stand-in for the avatar surfaces (inbox card, settled row, rail, filter
 *  dropdown) — renders whatever appearance the shared store currently holds. */
function AppearanceProbe({ testId }: { testId: string }) {
  const { customColor, imageUrl, imageVersion } = useProjectAppearance(PATH);
  return (
    <div
      data-testid={testId}
      data-color={customColor ?? ""}
      data-image={imageUrl ?? ""}
      data-version={imageVersion ?? ""}
    />
  );
}

function renderMenu(onRequestImageDialog = () => {}) {
  return render(
    <ProjectAppearanceMenu
      projectName="codemux"
      projectPath={PATH}
      onRequestImageDialog={onRequestImageDialog}
    />,
  );
}

/** Flush the store's async load so assertions see persisted values. */
async function flushLoad() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

beforeEach(() => {
  persisted = {};
  __resetProjectAppearanceStoreForTests();
  vi.mocked(dbSetUiState).mockClear();
  vi.mocked(dbGetUiState).mockClear();
});

afterEach(() => {
  cleanup();
});

describe("ProjectAppearanceMenu", () => {
  it("names the project it will affect, so the menu is unambiguous", async () => {
    renderMenu();
    await flushLoad();
    expect(screen.getByText(/codemux/)).toBeInTheDocument();
  });

  it("offers 'Set image…' when the project has no image yet", async () => {
    renderMenu();
    await flushLoad();
    expect(screen.getByRole("menuitem", { name: /Set image/ })).toBeInTheDocument();
  });

  it("offers 'Change image…' once an image is stored", async () => {
    persisted[`project.image:${PATH}`] = "codemux.com";
    renderMenu();
    await flushLoad();
    expect(
      screen.getByRole("menuitem", { name: /Change image/ }),
    ).toBeInTheDocument();
  });

  it("opens the image dialog rather than saving inline", async () => {
    const onRequest = vi.fn();
    renderMenu(onRequest);
    await flushLoad();
    fireEvent.click(screen.getByRole("menuitem", { name: /Set image/ }));
    expect(onRequest).toHaveBeenCalledTimes(1);
    expect(dbSetUiState).not.toHaveBeenCalled();
  });

  it("persists a picked color under the project's color key", async () => {
    renderMenu();
    await flushLoad();
    fireEvent.click(screen.getByRole("menuitem", { name: /Teal/ }));
    expect(dbSetUiState).toHaveBeenCalledWith(`project.color:${PATH}`, "#14b8a6");
  });

  it("clears the color by writing an empty value, not deleting the key", async () => {
    persisted[`project.color:${PATH}`] = "#14b8a6";
    renderMenu();
    await flushLoad();
    fireEvent.click(screen.getByRole("menuitem", { name: /Default/ }));
    expect(dbSetUiState).toHaveBeenCalledWith(`project.color:${PATH}`, "");
  });

  it("loads and check-marks the stored color", async () => {
    persisted[`project.color:${PATH}`] = "#14b8a6";
    renderMenu();
    await flushLoad();
    const teal = screen.getByRole("menuitem", { name: /Teal/ });
    // The check icon is the only extra child on the selected entry.
    expect(teal.querySelector("svg")).toBeTruthy();
  });
});

describe("project appearance propagation", () => {
  it("repaints every surface of the same project on a color write", async () => {
    render(
      <>
        <AppearanceProbe testId="card" />
        <AppearanceProbe testId="rail" />
        <ProjectAppearanceMenu
          projectName="codemux"
          projectPath={PATH}
          onRequestImageDialog={() => {}}
        />
      </>,
    );
    await flushLoad();

    expect(screen.getByTestId("card")).toHaveAttribute("data-color", "");
    expect(screen.getByTestId("rail")).toHaveAttribute("data-color", "");

    fireEvent.click(screen.getByRole("menuitem", { name: /Purple/ }));

    // Both surfaces update immediately — this is the regression the old
    // per-component hook had: a write only showed up on the next remount.
    expect(screen.getByTestId("card")).toHaveAttribute("data-color", "#a855f7");
    expect(screen.getByTestId("rail")).toHaveAttribute("data-color", "#a855f7");
  });

  it("reads a project's persisted appearance only once across many avatars", async () => {
    render(
      <>
        <AppearanceProbe testId="a" />
        <AppearanceProbe testId="b" />
        <AppearanceProbe testId="c" />
      </>,
    );
    await flushLoad();
    // Three keys (color, image, image version) — once, not once per avatar.
    expect(vi.mocked(dbGetUiState)).toHaveBeenCalledTimes(3);
  });

  it("surfaces a saved image with a fresh cache-bust token", async () => {
    render(
      <>
        <AppearanceProbe testId="card" />
      </>,
    );
    await flushLoad();

    act(() => {
      useProjectAppearanceStore.getState().setImage(PATH, "codemux.com");
    });

    const card = screen.getByTestId("card");
    expect(card).toHaveAttribute("data-image", "codemux.com");
    // A token must accompany the image so a changed favicon re-fetches
    // instead of the WebView serving the same cached bytes forever.
    expect(card.getAttribute("data-version")).toMatch(/^\d+$/);
    expect(dbSetUiState).toHaveBeenCalledWith(
      `project.image:${PATH}`,
      "codemux.com",
    );
  });

  it("clears image and token together when the image is removed", async () => {
    persisted[`project.image:${PATH}`] = "codemux.com";
    persisted[`project.image.v:${PATH}`] = "123";
    render(<AppearanceProbe testId="card" />);
    await flushLoad();
    expect(screen.getByTestId("card")).toHaveAttribute("data-image", "codemux.com");

    act(() => {
      useProjectAppearanceStore.getState().setImage(PATH, null);
    });

    const card = screen.getByTestId("card");
    expect(card).toHaveAttribute("data-image", "");
    expect(card).toHaveAttribute("data-version", "");
    expect(dbSetUiState).toHaveBeenCalledWith(`project.image:${PATH}`, "");
    expect(dbSetUiState).toHaveBeenCalledWith(`project.image.v:${PATH}`, "");
  });

  it("does not let a slow load clobber a color the user just picked", async () => {
    // Store holds a stale value; the user picks a new one before the read
    // resolves. The in-flight read must not win.
    persisted[`project.color:${PATH}`] = "#ef4444";
    render(<AppearanceProbe testId="card" />);

    act(() => {
      useProjectAppearanceStore.getState().setColor(PATH, "#3b82f6");
    });
    await flushLoad();

    expect(screen.getByTestId("card")).toHaveAttribute("data-color", "#3b82f6");
  });
});
