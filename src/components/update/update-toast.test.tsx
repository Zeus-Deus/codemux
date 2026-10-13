/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import { Toaster } from "@/components/ui/sonner";
import type { useUpdateChecker } from "@/hooks/use-update-checker";

type Checker = ReturnType<typeof useUpdateChecker>;
let checker: Checker;

vi.mock("@/hooks/use-update-checker", () => ({
  useUpdateChecker: () => checker,
}));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

import { openUrl } from "@tauri-apps/plugin-opener";
import { UpdateToast } from "./update-toast";

function base(overrides: Partial<Checker>): Checker {
  return {
    state: "update-available",
    updateVersion: "0.24.0",
    downloadProgress: 0,
    canAutoUpdate: true,
    packageFormat: "appimage",
    errorMessage: null,
    startDownload: vi.fn(),
    installAndRestart: vi.fn(),
    retry: vi.fn(),
    dismiss: vi.fn(),
    dismissed: false,
    isRemote: false,
    remoteClientsConnected: false,
    requestDesktopUpdate: vi.fn(),
    updateRequested: false,
    ...overrides,
  };
}

function renderToast(overrides: Partial<Checker>) {
  checker = base(overrides);
  return render(
    <>
      <Toaster />
      <UpdateToast />
    </>,
  );
}

afterEach(() => {
  toast.dismiss();
  cleanup();
  vi.clearAllMocks();
});

describe("UpdateToast", () => {
  it("stays up on a failed download and says why", async () => {
    renderToast({
      state: "error",
      errorMessage: "Download request failed with status: 404 Not Found",
    });

    expect(await screen.findByText("Update failed")).toBeInTheDocument();
    expect(
      screen.getByText("Download request failed with status: 404 Not Found"),
    ).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(checker.retry).toHaveBeenCalledTimes(1);

    await userEvent.click(
      screen.getByRole("button", { name: "Download manually" }),
    );
    expect(openUrl).toHaveBeenCalledWith(
      "https://github.com/Zeus-Deus/codemux/releases/tag/v0.24.0",
    );

    await userEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(checker.dismiss).toHaveBeenCalledTimes(1);
  });

  it("lets a ready update wait until later", async () => {
    renderToast({ state: "ready" });

    await userEvent.click(await screen.findByRole("button", { name: "Later" }));
    expect(checker.dismiss).toHaveBeenCalledTimes(1);
    expect(checker.installAndRestart).not.toHaveBeenCalled();
  });

  it("shows the progress percent with tabular figures", async () => {
    renderToast({ state: "downloading", downloadProgress: 42 });

    expect(await screen.findByText("42%")).toHaveClass("tabular-nums");
  });

  it("links release notes beside the in-app install", async () => {
    renderToast({});

    await userEvent.click(
      await screen.findByRole("button", { name: "What's new" }),
    );
    expect(openUrl).toHaveBeenCalledWith(
      "https://github.com/Zeus-Deus/codemux/releases/tag/v0.24.0",
    );
    expect(checker.startDownload).not.toHaveBeenCalled();
  });

  it("gives a pacman install its update command instead of an installer", async () => {
    renderToast({ canAutoUpdate: false, packageFormat: "pacman" });

    expect(await screen.findByText("yay -S codemux-bin")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /Install/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Copy command" }),
    ).toBeInTheDocument();
  });

  it("sends other package-manager installs to the release page", async () => {
    renderToast({ canAutoUpdate: false, packageFormat: "other" });

    await userEvent.click(
      await screen.findByRole("button", { name: "Download" }),
    );
    expect(openUrl).toHaveBeenCalledWith(
      "https://github.com/Zeus-Deus/codemux/releases/tag/v0.24.0",
    );
    expect(checker.startDownload).not.toHaveBeenCalled();
  });
});
