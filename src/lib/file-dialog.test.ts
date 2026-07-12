import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

// ── Mock Tauri commands + toast + remote path picker ──
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
vi.mock("@/components/remote/remote-path-picker-store", () => ({
  openRemotePathPicker: vi.fn(),
}));

import { pickFolderDialog, pickFilesDialog } from "@/tauri/commands";
import { toast } from "@/lib/toast";
import { openRemotePathPicker } from "@/components/remote/remote-path-picker-store";
import { pickFolder, pickFiles, NO_FILE_PICKER_BACKEND } from "./file-dialog";

const mockPickFolderDialog = vi.mocked(pickFolderDialog);
const mockPickFilesDialog = vi.mocked(pickFilesDialog);
const mockToastError = vi.mocked(toast.error);
const mockOpenRemotePathPicker = vi.mocked(openRemotePathPicker);

/** Toggle the web-remote flag `isRemoteClient()` reads. */
function setRemote(on: boolean): void {
  (window as unknown as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ =
    on;
}

beforeEach(() => {
  vi.clearAllMocks();
  setRemote(false);
});

afterEach(() => {
  delete (window as unknown as { __CODEMUX_REMOTE__?: boolean })
    .__CODEMUX_REMOTE__;
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

// ── Web remote client routing ──
//
// On the web remote client there is no native OS dialog to reach, so the
// chokepoint must route to the in-app path browser instead of invoking the
// `pick_*_dialog` commands, while returning the identical shape.
describe("remote client routing", () => {
  it("pickFolder uses the path browser, not the native command", async () => {
    setRemote(true);
    mockOpenRemotePathPicker.mockResolvedValue(["/home/dev/projects/codemux"]);

    await expect(pickFolder("Open project")).resolves.toBe(
      "/home/dev/projects/codemux",
    );

    expect(mockOpenRemotePathPicker).toHaveBeenCalledWith(
      "folder",
      "Open project",
    );
    expect(mockPickFolderDialog).not.toHaveBeenCalled();
  });

  it("pickFolder maps a cancel (null) from the path browser to null", async () => {
    setRemote(true);
    mockOpenRemotePathPicker.mockResolvedValue(null);

    await expect(pickFolder("Open project")).resolves.toBeNull();
    expect(mockPickFolderDialog).not.toHaveBeenCalled();
  });

  it("pickFolder maps an empty selection to null", async () => {
    setRemote(true);
    mockOpenRemotePathPicker.mockResolvedValue([]);

    await expect(pickFolder("Open project")).resolves.toBeNull();
  });

  it("pickFiles uses the path browser in multi-file mode", async () => {
    setRemote(true);
    mockOpenRemotePathPicker.mockResolvedValue(["/a.png", "/b.png"]);

    await expect(pickFiles("Attach files")).resolves.toEqual([
      "/a.png",
      "/b.png",
    ]);

    expect(mockOpenRemotePathPicker).toHaveBeenCalledWith(
      "files",
      "Attach files",
    );
    expect(mockPickFilesDialog).not.toHaveBeenCalled();
  });

  it("pickFiles maps a cancel (null) to an empty list", async () => {
    setRemote(true);
    mockOpenRemotePathPicker.mockResolvedValue(null);

    await expect(pickFiles("Attach files")).resolves.toEqual([]);
    expect(mockPickFilesDialog).not.toHaveBeenCalled();
  });

  it("pickFiles falls back to a default title when none is given", async () => {
    setRemote(true);
    mockOpenRemotePathPicker.mockResolvedValue([]);

    await pickFiles();

    expect(mockOpenRemotePathPicker).toHaveBeenCalledWith(
      "files",
      "Select files",
    );
  });

  it("desktop (flag off) still calls the native command, never the picker", async () => {
    // Default beforeEach leaves the flag off.
    mockPickFolderDialog.mockResolvedValue("/native/path");

    await expect(pickFolder("Open project")).resolves.toBe("/native/path");
    expect(mockOpenRemotePathPicker).not.toHaveBeenCalled();
  });
});
