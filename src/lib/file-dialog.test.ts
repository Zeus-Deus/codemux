import { describe, it, expect, vi, beforeEach } from "vitest";

// ── Mock Tauri commands + toast ──
vi.mock("@/tauri/commands", () => ({
  pickFolderDialog: vi.fn(),
  pickFilesDialog: vi.fn(),
}));
vi.mock("@/lib/toast", () => ({
  toast: {
    error: vi.fn(),
    info: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
  },
}));

import { pickFolderDialog, pickFilesDialog } from "@/tauri/commands";
import { toast } from "@/lib/toast";
import { pickFolder, pickFiles, NO_FILE_PICKER_BACKEND } from "./file-dialog";

const mockPickFolderDialog = vi.mocked(pickFolderDialog);
const mockPickFilesDialog = vi.mocked(pickFilesDialog);
const mockToastError = vi.mocked(toast.error);

beforeEach(() => {
  vi.clearAllMocks();
});

describe("pickFolder", () => {
  it("passes through a selected path", async () => {
    mockPickFolderDialog.mockResolvedValue("/home/user/project");
    await expect(pickFolder("Open project")).resolves.toBe(
      "/home/user/project",
    );
    expect(mockToastError).not.toHaveBeenCalled();
  });

  it("passes through a cancel (null) without toasting", async () => {
    mockPickFolderDialog.mockResolvedValue(null);
    await expect(pickFolder("Open project")).resolves.toBeNull();
    expect(mockToastError).not.toHaveBeenCalled();
  });

  it("surfaces the cause-specific backend message as the toast description", async () => {
    // The Rust preflight rejects with the marker plus a cause-specific
    // remediation (issue #95). The wrapper must surface that detail
    // verbatim, not a fixed "install packages" hint.
    mockPickFolderDialog.mockRejectedValue(
      `${NO_FILE_PICKER_BACKEND}: The xdg-desktop-portal is installed but is not starting in this session. Export it to D-Bus and restart Codemux. Alternatively, install zenity and Codemux will use it as a fallback file picker.`,
    );

    await expect(pickFolder("Open project")).resolves.toBeNull();

    expect(mockToastError).toHaveBeenCalledTimes(1);
    const [headline, opts] = mockToastError.mock.calls[0];
    expect(headline).toMatch(/file picker/i);
    // The marker prefix is stripped; the actionable detail survives.
    expect(opts?.description).not.toContain(NO_FILE_PICKER_BACKEND);
    expect(opts?.description).toMatch(/not starting in this session/);
    expect(opts?.description).toMatch(/xdg-desktop-portal/);
    expect(opts?.description).toMatch(/zenity/);
  });

  it("surfaces other dialog errors as a generic toast and resolves null", async () => {
    mockPickFolderDialog.mockRejectedValue("channel closed");
    await expect(pickFolder("Open project")).resolves.toBeNull();
    expect(mockToastError).toHaveBeenCalledTimes(1);
    const [, opts] = mockToastError.mock.calls[0];
    expect(opts?.description).toContain("channel closed");
  });
});

describe("pickFiles", () => {
  it("passes through selected files", async () => {
    mockPickFilesDialog.mockResolvedValue(["/a.png", "/b.png"]);
    await expect(pickFiles("Attach files")).resolves.toEqual([
      "/a.png",
      "/b.png",
    ]);
    expect(mockToastError).not.toHaveBeenCalled();
  });

  it("resolves empty list and toasts on missing backend", async () => {
    mockPickFilesDialog.mockRejectedValue(
      `${NO_FILE_PICKER_BACKEND}: cannot open a file dialog.`,
    );
    await expect(pickFiles("Attach files")).resolves.toEqual([]);
    expect(mockToastError).toHaveBeenCalledTimes(1);
  });
});
