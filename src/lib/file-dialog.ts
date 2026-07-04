/**
 * Never-silent wrappers around the native file dialog commands.
 *
 * Issue #95: on Linux the dialog backend is portal-first with a
 * zenity fallback, and on minimal window-manager setups (i3, dwm,
 * ...) neither may exist. The Rust side preflights that and rejects
 * with a marker error — these wrappers turn it into an actionable
 * toast and resolve like a cancel, so call sites keep their simple
 * "null/empty means no selection" contract while the user actually
 * learns what to install.
 *
 * UI call sites should import from here, not call the raw
 * `pickFolderDialog`/`pickFilesDialog` commands directly.
 */

import { pickFolderDialog, pickFilesDialog } from "@/tauri/commands";
import { toast } from "@/lib/toast";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { openRemotePathPicker } from "@/components/remote/remote-path-picker-store";

/** Marker prefix the Rust preflight puts on the error when no file
 *  picker backend exists. Kept in sync with `NO_BACKEND_MARKER` in
 *  `src-tauri/src/dialog_preflight.rs`. */
export const NO_FILE_PICKER_BACKEND = "NO_FILE_PICKER_BACKEND";

function describeError(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  return String(err);
}

function surfaceDialogError(err: unknown): void {
  const message = describeError(err);
  if (message.includes(NO_FILE_PICKER_BACKEND)) {
    // The Rust preflight builds a cause-specific, actionable message
    // (portal not installed vs installed-but-not-starting, plus the
    // zenity fallback). Surface it verbatim instead of a fixed
    // "install packages" hint that's wrong when the packages are
    // already there but the portal just isn't starting (issue #95).
    const markerAt = message.indexOf(NO_FILE_PICKER_BACKEND);
    const detail = message
      .slice(markerAt + NO_FILE_PICKER_BACKEND.length)
      .replace(/^[:\s]+/, "")
      .trim();
    toast.error("Can't open a file picker on this system", {
      description:
        detail ||
        "Install xdg-desktop-portal and xdg-desktop-portal-gtk, then restart your session. Installing zenity also works.",
    });
  } else {
    toast.error("Could not open the file dialog", { description: message });
  }
  console.error("[file-dialog]", err);
}

/** Folder picker that never throws: resolves the chosen path, or
 *  `null` on cancel AND on failure — failures additionally surface
 *  as an error toast instead of a silent no-op.
 *
 *  On the web remote client there is no native OS dialog to reach, so
 *  this routes to the in-app path browser (see {@link openRemotePathPicker})
 *  which walks the *host* filesystem and returns the same
 *  `absolute path | null` shape the native command does. */
export async function pickFolder(title: string): Promise<string | null> {
  if (isRemoteClient()) {
    const paths = await openRemotePathPicker("folder", title);
    return paths && paths.length > 0 ? paths[0] : null;
  }
  try {
    return await pickFolderDialog(title);
  } catch (err) {
    surfaceDialogError(err);
    return null;
  }
}

/** Multi-file picker with the same never-throw contract as
 *  {@link pickFolder}; failure resolves to an empty list. On the web
 *  remote client this routes to the in-app path browser in multi-file
 *  mode, returning the same `absolute path[]` shape as the native
 *  command (empty on cancel). */
export async function pickFiles(title?: string): Promise<string[]> {
  if (isRemoteClient()) {
    const paths = await openRemotePathPicker(
      "files",
      title ?? "Select files",
    );
    return paths ?? [];
  }
  try {
    return await pickFilesDialog(title);
  } catch (err) {
    surfaceDialogError(err);
    return [];
  }
}
